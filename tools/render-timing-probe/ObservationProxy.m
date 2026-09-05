// Instance-local, opt-in instrumentation for the isolated MetalFX experiment.
#import "ObservationProxy.h"
#import <objc/runtime.h>
#include <dlfcn.h>
#include <execinfo.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

enum { ObservationMaximumUnknownStacks=4, ObservationMaximumStackFrames=32,
       ObservationMaximumDiagnosticStringBytes=1024 };

static NSString *ObservationAddress(uintptr_t address) {
    return [NSString stringWithFormat:@"0x%llx",(unsigned long long)address];
}

static id ObservationDiagnosticString(const char *value) {
    if (!value) return [NSNull null];
    // Missing, overlong or invalid UTF-8 metadata stays explicitly unavailable.
    size_t length=strnlen(value,ObservationMaximumDiagnosticStringBytes+1);
    if (length>ObservationMaximumDiagnosticStringBytes) return [NSNull null];
    return [[NSString alloc] initWithBytes:value length:length encoding:NSUTF8StringEncoding]?:[NSNull null];
}

static NSDictionary *ObservationUnknownStack(NSString *selector,NSUInteger selectorOrdinal,
                                             NSUInteger captureOrdinal,id proxy,id target) {
    void *addresses[ObservationMaximumStackFrames];
    int count=backtrace(addresses,ObservationMaximumStackFrames);
    NSMutableArray *frames=[NSMutableArray new];
    for (int index=0;index<count;index++) {
        uintptr_t pc=(uintptr_t)addresses[index];Dl_info info={0};
        BOOL found=dladdr(addresses[index],&info)!=0;
        uintptr_t base=(uintptr_t)info.dli_fbase,symbol=(uintptr_t)info.dli_saddr;
        [frames addObject:@{@"program_counter":ObservationAddress(pc),
            @"image_path":found?ObservationDiagnosticString(info.dli_fname):[NSNull null],
            @"image_load_address":found && info.dli_fbase?ObservationAddress(base):[NSNull null],
            @"image_offset":found && info.dli_fbase && pc>=base?ObservationAddress(pc-base):[NSNull null],
            @"symbol":found?ObservationDiagnosticString(info.dli_sname):[NSNull null],
            @"symbol_offset":found && info.dli_saddr && pc>=symbol?ObservationAddress(pc-symbol):[NSNull null]}];
    }
    return @{@"selector":ObservationDiagnosticString(selector.UTF8String),@"selector_call_ordinal":@(selectorOrdinal),@"capture_ordinal":@(captureOrdinal),
        // Runtime queries avoid sending extra selectors through either delegate.
        @"proxy_runtime_class":ObservationDiagnosticString(class_getName(object_getClass(proxy))),
        @"target_runtime_class":ObservationDiagnosticString(class_getName(object_getClass(target))),
        @"frames":[frames copy],@"frame_limit_reached":[NSNumber numberWithBool:count==ObservationMaximumStackFrames]};
}

@interface ObservedEncoder : NSObject
@property(nonatomic) NSUInteger ordinal;
@property(nonatomic) NSUInteger sampleCount;
@property(nonatomic,copy) NSString *family;
@property(nonatomic,copy) NSString *selector;
@property(nonatomic,copy) NSString *label;
@property(nonatomic,strong) id encoder;
@property(nonatomic,strong) id<MTLCounterSampleBuffer> counter;
@property(nonatomic,copy) NSArray<NSNumber *> *ticks;
@end
@implementation ObservedEncoder
@end

@interface ObservationLedger ()
@property(nonatomic,strong) NSLock *lock;
@property(nonatomic,copy) NSDictionary *identity;
@property(nonatomic,copy) NSString *expectedLabel;
@property(nonatomic) ObservationMode mode;
@property(nonatomic) NSUInteger maximumEncoders;
@property(nonatomic,copy) ObservationCounterFactory factory;
@property(nonatomic,strong) NSMutableArray<ObservedEncoder *> *encoders;
@property(nonatomic,strong) NSMutableArray<NSString *> *selectors;
@property(nonatomic,strong) NSMutableOrderedSet<NSString *> *errors;
@property(nonatomic) BOOL captureUnknownStacks;
@property(nonatomic) NSUInteger totalUnknownSelectorCalls;
@property(nonatomic) NSUInteger unknownStackAttempts;
@property(nonatomic) NSUInteger unknownStackFailures;
@property(nonatomic,strong) NSMutableArray<NSDictionary *> *unknownStacks;
@property(nonatomic) NSUInteger totalEncoders;
@property(nonatomic) NSUInteger totalSelectors;
@property(nonatomic) NSUInteger requestedSamples;
@property(nonatomic) BOOL sealed;
@property(nonatomic) BOOL completed;
@property(nonatomic,copy) NSString *sealedLabel;
@property(nonatomic,strong) NSNumber *sealedStatus;
@property(nonatomic,strong) NSNumber *completedStatus;
- (NSUInteger)noteSelector:(NSString *)selector supported:(BOOL)supported;
- (void)captureUnknownSelector:(NSString *)selector ordinal:(NSUInteger)ordinal proxy:(id)proxy target:(id)target;
- (ObservedEncoder *)beginEncoder:(NSString *)family selector:(NSString *)selector;
- (id<MTLCounterSampleBuffer>)sampleFor:(ObservedEncoder *)record;
- (void)finishEncoder:(ObservedEncoder *)record encoder:(id)encoder;
- (void)unavailable:(NSString *)error;
- (NSDictionary *)snapshotLocked;
@end

@implementation ObservationLedger
- (instancetype)initWithIdentity:(NSDictionary *)identity expectedLabel:(NSString *)label
                           mode:(ObservationMode)mode maximumEncoders:(NSUInteger)maximum
                 counterFactory:(ObservationCounterFactory)factory {
    self=[super init];
    if (self) {
        _lock=[NSLock new];_identity=[[NSDictionary alloc] initWithDictionary:identity copyItems:YES];
        _expectedLabel=[label copy];_mode=mode;_maximumEncoders=MIN(maximum,32);_factory=[factory copy];
        _encoders=[NSMutableArray new];_selectors=[NSMutableArray new];_errors=[NSMutableOrderedSet new];
        const char *capture=getenv("USHAS_OBSERVATION_CAPTURE_UNKNOWN_STACK");
        _captureUnknownStacks=capture && strcmp(capture,"1")==0;_unknownStacks=[NSMutableArray new];
        if (maximum==0 || maximum>32) [_errors addObject:@"invalid_encoder_limit"];
    }
    return self;
}
- (void)unavailable:(NSString *)error { [self.lock lock];[self.errors addObject:error];[self.lock unlock]; }
- (NSUInteger)noteSelector:(NSString *)selector supported:(BOOL)supported {
    [self.lock lock];
    self.totalSelectors++;
    if (self.selectors.count<256) [self.selectors addObject:selector]; else [self.errors addObject:@"selector_limit"];
    if (!supported) { self.totalUnknownSelectorCalls++;[self.errors addObject:@"unsupported_selector"]; }
    NSUInteger ordinal=self.totalSelectors;
    [self.lock unlock];return ordinal;
}
- (void)captureUnknownSelector:(NSString *)selector ordinal:(NSUInteger)ordinal proxy:(id)proxy target:(id)target {
    [self.lock lock];
    if (!self.captureUnknownStacks || self.unknownStackAttempts>=ObservationMaximumUnknownStacks) {
        [self.lock unlock];return;
    }
    NSUInteger captureOrdinal=++self.unknownStackAttempts;
    [self.lock unlock];
    // Reserve under the lock, but unwind/symbolize outside it. This deliberately
    // perturbs CPU execution and is never a governor or overhead measurement.
    @try {
        NSDictionary *record=ObservationUnknownStack(selector,ordinal,captureOrdinal,proxy,target);
        [self.lock lock];
        @try {
            if ([record[@"frames"] count]) [self.unknownStacks addObject:record];
            else self.unknownStackFailures++;
        } @finally { [self.lock unlock]; }
    } @catch (NSException *exception) {
        (void)exception;[self.lock lock];self.unknownStackFailures++;[self.lock unlock];
    }
}
- (ObservedEncoder *)beginEncoder:(NSString *)family selector:(NSString *)selector {
    [self noteSelector:selector supported:YES];
    [self.lock lock];
    self.totalEncoders++;
    if (self.sealed || self.completed) {
        [self.errors addObject:@"encoder_after_seal_or_completion"];[self.lock unlock];return nil;
    }
    if (self.encoders.count>=self.maximumEncoders) {
        [self.errors addObject:@"encoder_limit"];[self.lock unlock];return nil;
    }
    ObservedEncoder *record=[ObservedEncoder new];record.ordinal=self.totalEncoders;
    record.family=family;record.selector=selector;record.sampleCount=[family isEqual:@"render"]?4:2;
    [self.encoders addObject:record];[self.lock unlock];return record;
}
- (id<MTLCounterSampleBuffer>)sampleFor:(ObservedEncoder *)record {
    if (!record || self.mode!=ObservationModeCounters) return nil;
    [self.lock lock];
    if (self.sealed || self.completed || self.requestedSamples+record.sampleCount>128) {
        [self.errors addObject:@"sample_limit_or_closed_inventory"];[self.lock unlock];return nil;
    }
    self.requestedSamples+=record.sampleCount;
    NSError *error=nil;
    NSString *label=[NSString stringWithFormat:@"%@/encoder=%lu/%@",self.expectedLabel,(unsigned long)record.ordinal,record.family];
    record.counter=self.factory(label,record.sampleCount,&error);
    if (!record.counter) [self.errors addObject:@"counter_allocation_failed"];
    id<MTLCounterSampleBuffer> counter=record.counter;
    [self.lock unlock];return counter;
}
- (void)finishEncoder:(ObservedEncoder *)record encoder:(id)encoder {
    if (!record) return;
    [self.lock lock];record.encoder=encoder;
    if (!encoder) [self.errors addObject:@"encoder_creation_failed"];
    [self.lock unlock];
}
- (void)sealCommandBuffer:(id<MTLCommandBuffer>)buffer {
    [self.lock lock];
    if (self.sealed) { [self.errors addObject:@"duplicate_seal"];[self.lock unlock];return; }
    self.sealed=YES;self.sealedLabel=[buffer.label copy];self.sealedStatus=@(buffer.status);
    if (![self.sealedLabel isEqual:self.expectedLabel]) [self.errors addObject:@"command_buffer_label_changed"];
    if (buffer.status!=MTLCommandBufferStatusNotEnqueued) [self.errors addObject:@"unexpected_submission"];
    if (self.encoders.count==0) [self.errors addObject:@"no_encoders_observed"];
    NSMutableSet *labels=[NSMutableSet new];
    for (ObservedEncoder *record in self.encoders) {
        record.label=[[record.encoder label] copy];
        if (record.label.length==0 || [labels containsObject:record.label]) [self.errors addObject:@"ambiguous_encoder_label"];
        if (record.label) [labels addObject:record.label];
        // Release real encoders before commit: they may retain the command buffer.
        record.encoder=nil;
    }
    [self.lock unlock];
}
- (NSDictionary *)completeCommandBuffer:(id<MTLCommandBuffer>)buffer {
    [self.lock lock];
    if (self.completed) {
        [self.errors addObject:@"duplicate_completion"];
        NSDictionary *result=[self snapshotLocked];[self.lock unlock];return result;
    }
    self.completed=YES;self.completedStatus=@(buffer.status);
    if (!self.sealed) [self.errors addObject:@"completion_before_seal"];
    if (buffer.status!=MTLCommandBufferStatusCompleted || buffer.error) [self.errors addObject:@"command_buffer_failed"];
    if (![buffer.label isEqual:self.expectedLabel]) [self.errors addObject:@"command_buffer_label_changed"];
    if (self.sealed && buffer.status==MTLCommandBufferStatusCompleted && !buffer.error && self.mode==ObservationModeCounters) {
        for (ObservedEncoder *record in self.encoders) {
            if (!record.counter) { [self.errors addObject:@"missing_counter_buffer"];continue; }
            @try {
                NSData *data=[record.counter resolveCounterRange:NSMakeRange(0,record.sampleCount)];
                if (data.length!=record.sampleCount*sizeof(MTLCounterResultTimestamp)) {
                    [self.errors addObject:@"counter_resolution_length"];record.counter=nil;continue;
                }
                const MTLCounterResultTimestamp *samples=data.bytes;
                NSMutableArray *ticks=[NSMutableArray new];
                for (NSUInteger index=0;index<record.sampleCount;index++) {
                    uint64_t tick=samples[index].timestamp;[ticks addObject:@(tick)];
                    if (tick==0 || tick==UINT64_MAX || (index%2 && tick<=samples[index-1].timestamp))
                        [self.errors addObject:@"invalid_counter_interval"];
                }
                record.ticks=[ticks copy];
            } @catch (NSException *exception) { (void)exception;[self.errors addObject:@"counter_resolution_exception"]; }
            record.counter=nil;
        }
    }
    NSDictionary *result=[self snapshotLocked];[self.lock unlock];return result;
}
- (NSDictionary *)snapshotLocked {
    NSMutableArray *encoders=[NSMutableArray new];
    for (ObservedEncoder *record in self.encoders) {
        [encoders addObject:@{@"ordinal":@(record.ordinal),@"family":record.family,@"factory_selector":record.selector,
            @"label":record.label?:[NSNull null],@"sample_count":@(self.mode==ObservationModeCounters?record.sampleCount:0),
            @"ticks":record.ticks?:[NSNull null]}];
    }
    return @{@"available":[NSNumber numberWithBool:self.sealed && self.completed && self.errors.count==0],@"validated_for_governor":@NO,
        @"identity":self.identity,@"observation_mode":self.mode==ObservationModeCounters?@"counters":@"calls",
        @"expected_command_buffer_label":self.expectedLabel,@"sealed_command_buffer_label":self.sealedLabel?:[NSNull null],
        @"sealed_command_buffer_status":self.sealedStatus?:[NSNull null],@"completed_command_buffer_status":self.completedStatus?:[NSNull null],
        @"sealed":[NSNumber numberWithBool:self.sealed],@"completed":[NSNumber numberWithBool:self.completed],@"errors":[self.errors.array copy],
        @"selectors":[self.selectors copy],@"total_selector_calls":@(self.totalSelectors),
        @"dropped_selector_records":@(self.totalSelectors-self.selectors.count),@"total_encoder_factories":@(self.totalEncoders),
        @"unknown_selector_diagnostics":@{@"enabled":[NSNumber numberWithBool:self.captureUnknownStacks],
            @"capture_phase":@"before_forwarding_unknown_invocation",@"maximum_records":@(ObservationMaximumUnknownStacks),
            @"maximum_stack_frames":@(ObservationMaximumStackFrames),@"maximum_string_bytes":@(ObservationMaximumDiagnosticStringBytes),
            @"total_unknown_selector_calls":@(self.totalUnknownSelectorCalls),@"capture_attempts":@(self.unknownStackAttempts),
            @"capture_failures":@(self.unknownStackFailures),
            @"not_captured_calls":@(self.totalUnknownSelectorCalls-self.unknownStacks.count),@"records":[self.unknownStacks copy]},
        @"requested_samples":@(self.requestedSamples),@"encoders":[encoders copy]};
}
- (NSDictionary *)snapshot { [self.lock lock];NSDictionary *result=[self snapshotLocked];[self.lock unlock];return result; }
@end

@interface ObservedCommandBuffer ()
@property(nonatomic,strong) id<MTLCommandBuffer> target;
@property(nonatomic,strong) ObservationLedger *ledger;
@end
@implementation ObservedCommandBuffer
+ (id<MTLCommandBuffer>)wrap:(id<MTLCommandBuffer>)buffer ledger:(ObservationLedger *)ledger {
    ObservedCommandBuffer *proxy=[ObservedCommandBuffer alloc];
    proxy.target=buffer;proxy.ledger=ledger;
    return (id<MTLCommandBuffer>)proxy;
}
- (BOOL)respondsToSelector:(SEL)selector {
    return class_getInstanceMethod([ObservedCommandBuffer class],selector)!=NULL || [(id)self.target respondsToSelector:selector];
}
- (BOOL)conformsToProtocol:(Protocol *)protocol {
    return class_conformsToProtocol([ObservedCommandBuffer class],protocol) || [(id)self.target conformsToProtocol:protocol];
}
- (NSMethodSignature *)methodSignatureForSelector:(SEL)selector { return [(id)self.target methodSignatureForSelector:selector]; }
- (void)forwardInvocation:(NSInvocation *)invocation {
    static NSSet *allowed;static dispatch_once_t once;
    dispatch_once(&once, ^{ allowed=[NSSet setWithArray:@[@"device",@"commandQueue",@"label",@"setLabel:",
        @"retainedReferences",@"errorOptions",@"kernelStartTime",@"kernelEndTime",@"GPUStartTime",@"GPUEndTime",
        @"status",@"error",@"logs",@"addCompletedHandler:",@"addScheduledHandler:",@"pushDebugGroup:",@"popDebugGroup"]]; });
    NSString *selector=NSStringFromSelector(invocation.selector);
    BOOL supported=[allowed containsObject:selector];
    NSUInteger ordinal=[self.ledger noteSelector:selector supported:supported];
    if (!supported) [self.ledger captureUnknownSelector:selector ordinal:ordinal proxy:self target:self.target];
    [invocation invokeWithTarget:self.target];
}
- (id<MTLRenderCommandEncoder>)renderCommandEncoderWithDescriptor:(MTLRenderPassDescriptor *)descriptor {
    ObservedEncoder *record=[self.ledger beginEncoder:@"render" selector:NSStringFromSelector(_cmd)];
    MTLRenderPassDescriptor *used=descriptor;
    if (record && self.ledger.mode==ObservationModeCounters) {
        if (descriptor.sampleBufferAttachments[0].sampleBuffer) [self.ledger unavailable:@"sample_attachment_occupied"];
        else {
            id<MTLCounterSampleBuffer> counter=[self.ledger sampleFor:record];
            if (counter) {
                used=[descriptor copy];MTLRenderPassSampleBufferAttachmentDescriptor *attachment=used.sampleBufferAttachments[0];
                attachment.sampleBuffer=counter;attachment.startOfVertexSampleIndex=0;attachment.endOfVertexSampleIndex=1;
                attachment.startOfFragmentSampleIndex=2;attachment.endOfFragmentSampleIndex=3;
            }
        }
    }
    id<MTLRenderCommandEncoder> encoder=[self.target renderCommandEncoderWithDescriptor:used];
    [self.ledger finishEncoder:record encoder:encoder];return encoder;
}
- (id<MTLComputeCommandEncoder>)compute:(MTLComputePassDescriptor *)descriptor selector:(SEL)selector dispatchType:(MTLDispatchType)dispatchType {
    ObservedEncoder *record=[self.ledger beginEncoder:@"compute" selector:NSStringFromSelector(selector)];
    MTLComputePassDescriptor *used=nil;
    if (record && self.ledger.mode==ObservationModeCounters) {
        if (descriptor.sampleBufferAttachments[0].sampleBuffer) [self.ledger unavailable:@"sample_attachment_occupied"];
        else {
            id<MTLCounterSampleBuffer> counter=[self.ledger sampleFor:record];
            if (counter) {
                used=descriptor?[descriptor copy]:[MTLComputePassDescriptor computePassDescriptor];
                if (!descriptor) used.dispatchType=dispatchType;
                used.sampleBufferAttachments[0].sampleBuffer=counter;
                used.sampleBufferAttachments[0].startOfEncoderSampleIndex=0;used.sampleBufferAttachments[0].endOfEncoderSampleIndex=1;
            }
        }
    }
    id<MTLComputeCommandEncoder> encoder;
    if (used) encoder=[self.target computeCommandEncoderWithDescriptor:used];
    else if (selector==@selector(computeCommandEncoder)) encoder=[self.target computeCommandEncoder];
    else if (selector==@selector(computeCommandEncoderWithDispatchType:)) encoder=[self.target computeCommandEncoderWithDispatchType:dispatchType];
    else encoder=[self.target computeCommandEncoderWithDescriptor:descriptor];
    [self.ledger finishEncoder:record encoder:encoder];return encoder;
}
- (id<MTLComputeCommandEncoder>)computeCommandEncoder { return [self compute:nil selector:_cmd dispatchType:MTLDispatchTypeSerial]; }
- (id<MTLComputeCommandEncoder>)computeCommandEncoderWithDispatchType:(MTLDispatchType)type { return [self compute:nil selector:_cmd dispatchType:type]; }
- (id<MTLComputeCommandEncoder>)computeCommandEncoderWithDescriptor:(MTLComputePassDescriptor *)descriptor { return [self compute:descriptor selector:_cmd dispatchType:descriptor.dispatchType]; }
- (id<MTLBlitCommandEncoder>)blit:(MTLBlitPassDescriptor *)descriptor selector:(SEL)selector {
    ObservedEncoder *record=[self.ledger beginEncoder:@"blit" selector:NSStringFromSelector(selector)];
    MTLBlitPassDescriptor *used=nil;
    if (record && self.ledger.mode==ObservationModeCounters) {
        if (descriptor.sampleBufferAttachments[0].sampleBuffer) [self.ledger unavailable:@"sample_attachment_occupied"];
        else {
            id<MTLCounterSampleBuffer> counter=[self.ledger sampleFor:record];
            if (counter) {
                used=descriptor?[descriptor copy]:[MTLBlitPassDescriptor blitPassDescriptor];
                used.sampleBufferAttachments[0].sampleBuffer=counter;
                used.sampleBufferAttachments[0].startOfEncoderSampleIndex=0;used.sampleBufferAttachments[0].endOfEncoderSampleIndex=1;
            }
        }
    }
    id<MTLBlitCommandEncoder> encoder;
    if (used) encoder=[self.target blitCommandEncoderWithDescriptor:used];
    else if (selector==@selector(blitCommandEncoder)) encoder=[self.target blitCommandEncoder];
    else encoder=[self.target blitCommandEncoderWithDescriptor:descriptor];
    [self.ledger finishEncoder:record encoder:encoder];return encoder;
}
- (id<MTLBlitCommandEncoder>)blitCommandEncoder { return [self blit:nil selector:_cmd]; }
- (id<MTLBlitCommandEncoder>)blitCommandEncoderWithDescriptor:(MTLBlitPassDescriptor *)descriptor { return [self blit:descriptor selector:_cmd]; }
@end

BOOL ObservationCommitIfNeeded(id<MTLCommandBuffer> buffer) {
    MTLCommandBufferStatus status=buffer.status;
    if (status==MTLCommandBufferStatusNotEnqueued || status==MTLCommandBufferStatusEnqueued) { [buffer commit];return YES; }
    return NO;
}
