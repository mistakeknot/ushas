// CPU-only tests: fake delegates and descriptor objects; never creates an MTLDevice.
#import "ObservationProxy.h"

static NSUInteger Failures=0, Checks=0;
#define CHECK(condition,message) do { Checks++; if (!(condition)) { Failures++; fprintf(stderr,"FAIL line %d: %s\n",__LINE__,message); } } while (0)

@interface FakeEncoder : NSObject
@property(nonatomic,copy) NSString *label;
@end
@implementation FakeEncoder
@end

@interface FakeCounter : NSObject
@property(nonatomic) NSUInteger sampleCount;
@property(nonatomic,copy) NSString *label;
@property(nonatomic) NSUInteger resolves;
- (NSData *)resolveCounterRange:(NSRange)range;
@end
@implementation FakeCounter
- (NSData *)resolveCounterRange:(NSRange)range {
    self.resolves++;
    NSMutableData *data=[NSMutableData dataWithLength:range.length*sizeof(MTLCounterResultTimestamp)];
    MTLCounterResultTimestamp *ticks=data.mutableBytes;
    for (NSUInteger index=0;index<range.length;index++) ticks[index].timestamp=100+index*10;
    return data;
}
@end

@interface FakeBuffer : NSObject
@property(nonatomic,copy) NSString *label;
@property(nonatomic) MTLCommandBufferStatus status;
@property(nonatomic,strong) NSError *error;
@property(nonatomic,strong) id lastDescriptor;
@property(nonatomic,strong) NSMutableArray<NSString *> *selectors;
@property(nonatomic,strong) NSMutableArray<FakeEncoder *> *encoders;
@property(nonatomic) NSUInteger commits;
- (id)computeCommandEncoder;
- (id)computeCommandEncoderWithDispatchType:(MTLDispatchType)type;
- (id)computeCommandEncoderWithDescriptor:(MTLComputePassDescriptor *)descriptor;
- (id)renderCommandEncoderWithDescriptor:(MTLRenderPassDescriptor *)descriptor;
- (id)blitCommandEncoder;
- (id)blitCommandEncoderWithDescriptor:(MTLBlitPassDescriptor *)descriptor;
- (id)resourceStateCommandEncoder;
- (id)unknownFactory;
- (void)commit;
@end
@implementation FakeBuffer
- (BOOL)conformsToProtocol:(Protocol *)protocol { return protocol==@protocol(MTLCommandBuffer) || [super conformsToProtocol:protocol]; }
- (instancetype)init {
    self=[super init];
    if (self) { _label=@"proxy/frame=1/view=1/epoch=2/gen=3"; _selectors=[NSMutableArray new]; _encoders=[NSMutableArray new]; }
    return self;
}
- (id)encoder:(NSString *)selector descriptor:(id)descriptor {
    [self.selectors addObject:selector]; self.lastDescriptor=descriptor;
    FakeEncoder *encoder=[FakeEncoder new];
    encoder.label=[NSString stringWithFormat:@"framework-label-%lu",(unsigned long)self.encoders.count];
    [self.encoders addObject:encoder];return encoder;
}
- (id)computeCommandEncoder { return [self encoder:NSStringFromSelector(_cmd) descriptor:nil]; }
- (id)computeCommandEncoderWithDispatchType:(MTLDispatchType)type {
    MTLComputePassDescriptor *descriptor=[MTLComputePassDescriptor computePassDescriptor];descriptor.dispatchType=type;
    return [self encoder:NSStringFromSelector(_cmd) descriptor:descriptor];
}
- (id)computeCommandEncoderWithDescriptor:(MTLComputePassDescriptor *)descriptor { return [self encoder:NSStringFromSelector(_cmd) descriptor:descriptor]; }
- (id)renderCommandEncoderWithDescriptor:(MTLRenderPassDescriptor *)descriptor { return [self encoder:NSStringFromSelector(_cmd) descriptor:descriptor]; }
- (id)blitCommandEncoder { return [self encoder:NSStringFromSelector(_cmd) descriptor:nil]; }
- (id)blitCommandEncoderWithDescriptor:(MTLBlitPassDescriptor *)descriptor { return [self encoder:NSStringFromSelector(_cmd) descriptor:descriptor]; }
- (id)resourceStateCommandEncoder { return [self encoder:NSStringFromSelector(_cmd) descriptor:nil]; }
- (id)unknownFactory { return [self encoder:NSStringFromSelector(_cmd) descriptor:nil]; }
- (void)commit { self.commits++;self.status=MTLCommandBufferStatusCommitted; }
@end

static NSDictionary *Identity(void) { return @{@"frame":@1,@"view":@1,@"epoch":@2,@"generation":@3}; }
static ObservationLedger *Ledger(FakeBuffer *buffer,ObservationMode mode,NSUInteger cap,NSMutableArray *counters) {
    return [[ObservationLedger alloc] initWithIdentity:Identity() expectedLabel:buffer.label mode:mode maximumEncoders:cap
        counterFactory:^id<MTLCounterSampleBuffer>(NSString *label,NSUInteger count,NSError **error) {
            (void)error;FakeCounter *counter=[FakeCounter new];counter.label=label;counter.sampleCount=count;
            [counters addObject:counter];return (id<MTLCounterSampleBuffer>)counter;
        }];
}
static NSDictionary *Finish(ObservationLedger *ledger,FakeBuffer *buffer) {
    [ledger sealCommandBuffer:(id<MTLCommandBuffer>)buffer];buffer.status=MTLCommandBufferStatusCompleted;
    return [ledger completeCommandBuffer:(id<MTLCommandBuffer>)buffer];
}
static BOOL Error(NSDictionary *result,NSString *code) { return [result[@"errors"] containsObject:code]; }

static BOOL SerializedLedgerBooleans(NSDictionary *record) {
    NSData *data=[NSJSONSerialization dataWithJSONObject:record options:0 error:NULL];
    NSDictionary *decoded=data?[NSJSONSerialization JSONObjectWithData:data options:0 error:NULL]:nil;
    for (NSString *key in @[@"available",@"sealed",@"completed",@"validated_for_governor"]) {
        id value=decoded[key];
        if (!value || CFGetTypeID((__bridge CFTypeRef)value)!=CFBooleanGetTypeID()) return NO;
    }
    return YES;
}

int main(void) {
    @autoreleasepool {
        // Exact forwarding and source identity, without counters.
        FakeBuffer *buffer=[FakeBuffer new];NSMutableArray *counters=[NSMutableArray new];
        ObservationLedger *ledger=Ledger(buffer,ObservationModeCalls,32,counters);
        id<MTLCommandBuffer> proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];
        CHECK(SerializedLedgerBooleans(ledger.snapshot),"unsealed ledger JSON uses Boolean values even when unavailable");
        id encoder=[proxy computeCommandEncoder];
        CHECK(encoder==buffer.encoders.firstObject,"returns the real encoder, not an encoder proxy");
        CHECK([buffer.selectors.lastObject isEqual:@"computeCommandEncoder"],"calls mode preserves the default factory");
        CHECK(counters.count==0,"calls mode allocates no counters");
        CHECK([(id)proxy respondsToSelector:@selector(unknownFactory)],"respondsToSelector reflects the delegate");
        CHECK([(id)proxy conformsToProtocol:@protocol(MTLCommandBuffer)],"protocol conformance reflects the delegate");
        CHECK([(id)proxy methodSignatureForSelector:@selector(unknownFactory)]!=nil,"actual forwarding method signature comes from delegate");
        NSDictionary *result=Finish(ledger,buffer);
        CHECK([result[@"available"] boolValue],"calls inventory seals and completes");
        CHECK(SerializedLedgerBooleans(result),"completed ledger JSON uses Boolean values when available");
        CHECK([result[@"identity"] isEqual:Identity()],"original immutable identity retained");
        CHECK([result[@"encoders"] count]==1,"factory recorded exactly once");

        // Default and explicit concurrent dispatch conversion.
        for (NSNumber *concurrent in @[@NO,@YES]) {
            buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCounters,32,counters);
            proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];
            if (concurrent.boolValue) [proxy computeCommandEncoderWithDispatchType:MTLDispatchTypeConcurrent]; else [proxy computeCommandEncoder];
            MTLComputePassDescriptor *descriptor=buffer.lastDescriptor;
            CHECK(descriptor!=nil,"sampled default factory becomes an actual descriptor");
            CHECK(descriptor.dispatchType==(concurrent.boolValue?MTLDispatchTypeConcurrent:MTLDispatchTypeSerial),"dispatch type preserved");
            CHECK(descriptor.sampleBufferAttachments[0].sampleBuffer!=nil,"counter attached to actual encoder descriptor");
            CHECK(descriptor.sampleBufferAttachments[0].startOfEncoderSampleIndex==0 && descriptor.sampleBufferAttachments[0].endOfEncoderSampleIndex==1,"encoder boundary indices");
            result=Finish(ledger,buffer);
            CHECK([result[@"available"] boolValue],"complete counter observation");
            CHECK(counters.count==1 && [(FakeCounter *)counters.firstObject resolves]==1,"direct CPU resolution occurs once after completion");
        }

        // Copied render descriptors and all four boundaries preserve caller state.
        buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCounters,32,counters);
        proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];
        MTLRenderPassDescriptor *original=[MTLRenderPassDescriptor renderPassDescriptor];
        original.colorAttachments[0].loadAction=MTLLoadActionLoad;original.colorAttachments[0].storeAction=MTLStoreActionDontCare;
        [proxy renderCommandEncoderWithDescriptor:original];MTLRenderPassDescriptor *copy=buffer.lastDescriptor;
        CHECK(copy!=original,"framework descriptor is copied");
        CHECK(original.sampleBufferAttachments[0].sampleBuffer==nil,"original descriptor is not mutated");
        CHECK(copy.colorAttachments[0].loadAction==MTLLoadActionLoad && copy.colorAttachments[0].storeAction==MTLStoreActionDontCare,"load/store preserved");
        MTLRenderPassSampleBufferAttachmentDescriptor *attachment=copy.sampleBufferAttachments[0];
        CHECK(attachment.startOfVertexSampleIndex==0 && attachment.endOfVertexSampleIndex==1 && attachment.startOfFragmentSampleIndex==2 && attachment.endOfFragmentSampleIndex==3,"all four real render boundaries");
        result=Finish(ledger,buffer);CHECK([result[@"available"] boolValue],"render counter observation completes");

        // Explicit compute and both blit factories preserve descriptors and sample boundaries.
        buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCounters,32,counters);
        proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];
        MTLComputePassDescriptor *computeOriginal=[MTLComputePassDescriptor computePassDescriptor];
        computeOriginal.dispatchType=MTLDispatchTypeConcurrent;
        [proxy computeCommandEncoderWithDescriptor:computeOriginal];MTLComputePassDescriptor *computeCopy=buffer.lastDescriptor;
        CHECK(computeCopy!=computeOriginal && computeOriginal.sampleBufferAttachments[0].sampleBuffer==nil,"explicit compute descriptor copied without mutation");
        CHECK(computeCopy.dispatchType==MTLDispatchTypeConcurrent,"explicit compute dispatch type preserved");
        CHECK(computeCopy.sampleBufferAttachments[0].sampleBuffer!=nil && computeCopy.sampleBufferAttachments[0].startOfEncoderSampleIndex==0 && computeCopy.sampleBufferAttachments[0].endOfEncoderSampleIndex==1,"explicit compute actual boundary samples");
        result=Finish(ledger,buffer);CHECK([result[@"available"] boolValue],"explicit compute completes");
        for (NSNumber *explicitDescriptor in @[@NO,@YES]) {
            buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCounters,32,counters);
            proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];
            MTLBlitPassDescriptor *blitOriginal=[MTLBlitPassDescriptor blitPassDescriptor];
            if (explicitDescriptor.boolValue) [proxy blitCommandEncoderWithDescriptor:blitOriginal];else [proxy blitCommandEncoder];
            MTLBlitPassDescriptor *blitCopy=buffer.lastDescriptor;
            CHECK(blitCopy!=nil && blitCopy!=blitOriginal && blitOriginal.sampleBufferAttachments[0].sampleBuffer==nil,"blit default/copy leaves original untouched");
            CHECK(blitCopy.sampleBufferAttachments[0].sampleBuffer!=nil && blitCopy.sampleBufferAttachments[0].startOfEncoderSampleIndex==0 && blitCopy.sampleBufferAttachments[0].endOfEncoderSampleIndex==1,"actual blit boundary samples");
            result=Finish(ledger,buffer);CHECK([result[@"available"] boolValue],"blit counter observation completes");
        }

        // Seal must release the real encoder even while the ledger/proxy remain alive.
        buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCalls,32,counters);
        proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];
        __weak id weakEncoder;
        @autoreleasepool { id temporary=[proxy computeCommandEncoder];weakEncoder=temporary;[buffer.encoders removeAllObjects]; }
        CHECK(weakEncoder!=nil,"ledger retains encoder until its final label is captured");
        [ledger sealCommandBuffer:(id<MTLCommandBuffer>)buffer];
        CHECK(weakEncoder==nil,"seal releases real encoder to avoid command-buffer callback ownership cycles");
        for (NSNumber *missing in @[@NO,@YES]) {
            buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCalls,32,counters);
            proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];
            [proxy computeCommandEncoder];[proxy blitCommandEncoder];
            buffer.encoders.lastObject.label=missing.boolValue?nil:buffer.encoders.firstObject.label;
            result=Finish(ledger,buffer);
            CHECK(Error(result,@"ambiguous_encoder_label") && ![result[@"available"] boolValue],"missing/duplicate framework labels invalidate the trace join");
        }

        // Occupied slots preserve the original operation but invalidate coverage.
        buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCounters,32,counters);
        proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];
        MTLComputePassDescriptor *occupied=[MTLComputePassDescriptor computePassDescriptor];
        FakeCounter *existing=[FakeCounter new];existing.sampleCount=2;existing.label=@"existing";
        occupied.sampleBufferAttachments[0].sampleBuffer=(id<MTLCounterSampleBuffer>)existing;
        [proxy computeCommandEncoderWithDescriptor:occupied];result=Finish(ledger,buffer);
        CHECK(buffer.lastDescriptor==occupied,"conflict forwards the untouched original descriptor");
        CHECK(counters.count==0,"conflict does not allocate a replacement counter");
        CHECK(Error(result,@"sample_attachment_occupied") && ![result[@"available"] boolValue],"descriptor collision is unavailable");

        // Unsupported/unknown factories still forward, but can never supply a complete observation.
        for (NSString *selector in @[@"resourceStateCommandEncoder",@"unknownFactory"]) {
            buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCounters,32,counters);
            proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];
            if ([selector isEqual:@"unknownFactory"]) [(id)proxy unknownFactory]; else [proxy resourceStateCommandEncoder];
            result=Finish(ledger,buffer);
            CHECK([buffer.selectors.lastObject isEqual:selector],"unsupported operation forwards once");
            CHECK([result[@"selectors"] containsObject:selector],"forwardInvocation does not bypass selector inventory");
            CHECK(Error(result,@"unsupported_selector") && ![result[@"available"] boolValue],"unknown coverage fails closed");
        }

        buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCounters,1,counters);
        proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];
        [proxy computeCommandEncoder];[proxy blitCommandEncoder];result=Finish(ledger,buffer);
        CHECK(buffer.encoders.count==2,"overflow never suppresses rendering");
        CHECK([result[@"encoders"] count]==1 && counters.count==1,"inventory and counter storage stay bounded");
        CHECK(Error(result,@"encoder_limit") && ![result[@"available"] boolValue],"overflow invalidates the whole observation");

        // Completion cannot race ahead of a sealed inventory; no unsafe early resolution.
        buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCounters,32,counters);
        proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];[proxy computeCommandEncoder];
        buffer.status=MTLCommandBufferStatusCompleted;result=[ledger completeCommandBuffer:(id<MTLCommandBuffer>)buffer];
        CHECK(Error(result,@"completion_before_seal") && ![result[@"available"] boolValue],"early completion is unavailable");
        CHECK([(FakeCounter *)counters.firstObject resolves]==0,"early completion does not resolve unsealed samples");

        // Framework submission and rewritten labels are preserved as unavailable.
        for (NSNumber *status in @[@(MTLCommandBufferStatusEnqueued),@(MTLCommandBufferStatusCommitted)]) {
            buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCalls,32,counters);
            proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];[proxy computeCommandEncoder];
            buffer.status=status.unsignedIntegerValue;[ledger sealCommandBuffer:(id<MTLCommandBuffer>)buffer];
            CHECK(Error([ledger snapshot],@"unexpected_submission"),"framework enqueue/commit is unavailable at seal");
        }
        buffer=[FakeBuffer new];counters=[NSMutableArray new];ledger=Ledger(buffer,ObservationModeCalls,32,counters);
        proxy=[ObservedCommandBuffer wrap:(id<MTLCommandBuffer>)buffer ledger:ledger];[proxy computeCommandEncoder];
        buffer.label=@"rewritten";result=Finish(ledger,buffer);
        CHECK(Error(result,@"command_buffer_label_changed") && ![result[@"available"] boolValue],"rewritten owned label invalidates trace identity");
        for (NSNumber *status in @[@(MTLCommandBufferStatusNotEnqueued),@(MTLCommandBufferStatusEnqueued),@(MTLCommandBufferStatusCommitted),@(MTLCommandBufferStatusCompleted)]) {
            buffer=[FakeBuffer new];buffer.status=status.unsignedIntegerValue;
            BOOL shouldCommit=buffer.status<MTLCommandBufferStatusCommitted;
            CHECK(ObservationCommitIfNeeded((id<MTLCommandBuffer>)buffer)==shouldCommit,"owner commit decision follows actual status");
            CHECK(buffer.commits==(shouldCommit?1:0),"already committed buffers are never double-committed");
        }
        printf("%lu checks, %lu failures\n",(unsigned long)Checks,(unsigned long)Failures);
        return Failures?1:0;
    }
}
