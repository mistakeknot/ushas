// Bounded native-only MetalFX proxy experiment. Root owns builds and GPU execution.
#import "ObservationProxy.h"
#import <MetalFX/MetalFX.h>
#import <CoreGraphics/CoreGraphics.h>
#import <ImageIO/ImageIO.h>
#import <CommonCrypto/CommonDigest.h>
#import <mach/mach_time.h>
#include <fcntl.h>
#include <math.h>
#include <unistd.h>

static const NSUInteger InputWidth=160,InputHeight=90,OutputWidth=320,OutputHeight=180,Frames=16,Slots=2;
static uint64_t NowNS(void) {
    static mach_timebase_info_data_t info;static dispatch_once_t once;
    dispatch_once(&once, ^{ mach_timebase_info(&info); });
    return (uint64_t)(((__uint128_t)mach_absolute_time()*info.numer)/info.denom);
}
static NSString *Digest(const void *bytes,NSUInteger length) {
    unsigned char hash[CC_SHA256_DIGEST_LENGTH];CC_SHA256(bytes,(CC_LONG)length,hash);
    NSMutableString *text=[NSMutableString new];
    for (NSUInteger i=0;i<sizeof(hash);i++) [text appendFormat:@"%02x",hash[i]];
    return text;
}
static NSDictionary *BufferState(id<MTLCommandBuffer> buffer) {
    return @{@"status":@(buffer.status),@"label":buffer.label?:[NSNull null],@"error":buffer.error.description?:[NSNull null]};
}
static NSDictionary *Options(int argc,const char *argv[]) {
    if (argc!=7) return nil;
    NSMutableDictionary *options=[NSMutableDictionary new];
    NSSet *allowed=[NSSet setWithArray:@[@"--mode",@"--observe",@"--out"]];
    for (int i=1;i<argc;i+=2) {
        NSString *key=[NSString stringWithUTF8String:argv[i]],*value=[NSString stringWithUTF8String:argv[i+1]];
        if (![allowed containsObject:key] || options[key] || !value.length) return nil;
        options[key]=value;
    }
    if (![@[@"spatial",@"temporal"] containsObject:options[@"--mode"]] ||
        ![@[@"off",@"calls",@"counters"] containsObject:options[@"--observe"]]) return nil;
    return [options copy];
}

static NSString *Shaders=@"#include <metal_stdlib>\n"
"using namespace metal;\n"
"struct V { float4 position [[position]]; };\n"
"vertex V vert(uint id [[vertex_id]]) { const float2 p[3]={float2(-1,-1),float2(3,-1),float2(-1,3)};return {float4(p[id],0,1)}; }\n"
"fragment float4 input_color(V v [[stage_in]],constant uint &frame [[buffer(0)]]) { return float4(v.position.x/160.0f,v.position.y/90.0f,.2f+float(frame)*.02f,1); }\n"
"fragment float4 composition(V v [[stage_in]],texture2d<float,access::read> source [[texture(0)]],constant uint &frame [[buffer(0)]]) {\n"
" uint2 xy=uint2(v.position.xy); if(all(xy==uint2(0)))return float4(float(frame&255)/255.0f,float((frame>>8)&255)/255.0f,.5f,1);\n"
" return float4(source.read(xy).rgb,1); }\n";

@interface ProxyFrame : NSObject
@property(nonatomic,copy) NSDictionary *identity;
@property(nonatomic,copy) NSString *prefix;
@property(nonatomic,strong) id<MTLTexture> input;
@property(nonatomic,strong) id<MTLTexture> scaled;
@property(nonatomic,strong) id<MTLTexture> composed;
@property(nonatomic,strong) id<MTLTexture> depth;
@property(nonatomic,strong) id<MTLTexture> motion;
@property(nonatomic,strong) id<MTLTexture> exposure;
@property(nonatomic,strong) id<MTLBuffer> rawReadback;
@property(nonatomic,strong) id<MTLBuffer> pngReadback;
@property(nonatomic,strong) ObservationLedger *ledger;
@property(nonatomic,copy) NSDictionary *observation;
@property(nonatomic,copy) NSDictionary *pixelResult;
@property(nonatomic,copy) NSArray *setupFamilies;
@property(nonatomic,copy) NSDictionary *fxState;
@property(nonatomic,copy) NSDictionary *setupState;
@property(nonatomic,copy) NSDictionary *finishState;
@property(nonatomic) uint64_t admittedNS;
@property(nonatomic) uint64_t encodeStartNS;
@property(nonatomic) uint64_t encodeEndNS;
@property(nonatomic) uint64_t fxCallbackNS;
@property(nonatomic) uint64_t resolvedNS;
@property(nonatomic) uint64_t readbackCallbackNS;
@property(nonatomic) BOOL setupSucceeded;
@property(nonatomic) BOOL fxSucceeded;
@property(nonatomic) BOOL readbackSucceeded;
@property(nonatomic) BOOL ownerCommittedFx;
@property(nonatomic) BOOL delivered;
@end
@implementation ProxyFrame
@end

@interface ProxyProbe : NSObject
@property(nonatomic,copy) NSDictionary *options;
@property(nonatomic,copy) NSString *directory;
@property(nonatomic,strong) id<MTLDevice> device;
@property(nonatomic,strong) id<MTLCommandQueue> queue;
@property(nonatomic,strong) id<MTLCounterSet> timestampSet;
@property(nonatomic,strong) id<MTLRenderPipelineState> inputPipeline;
@property(nonatomic,strong) id<MTLRenderPipelineState> composePipeline;
@property(nonatomic,strong) id<MTLFXSpatialScaler> spatial;
@property(nonatomic,strong) id<MTLFXTemporalScaler> temporal;
@property(nonatomic,strong) dispatch_queue_t control;
@property(nonatomic,strong) dispatch_source_t timer;
@property(nonatomic,strong) NSMutableArray *slots;
@property(nonatomic,strong) NSMutableArray *generations;
@property(nonatomic,strong) NSMutableArray<ProxyFrame *> *retainedFrames;
@property(nonatomic) FILE *stream;
@property(nonatomic) uint64_t startedNS;
@property(nonatomic) NSUInteger admitted;
@property(nonatomic) NSUInteger completed;
@property(nonatomic) NSUInteger skipped;
@property(nonatomic) NSUInteger gpuFailures;
@property(nonatomic) NSUInteger pixelFailures;
@property(nonatomic) NSUInteger unavailable;
@property(nonatomic) BOOL stopped;
- (void)emit:(NSDictionary *)record;
- (void)finish:(int)code reason:(NSString *)reason;
- (BOOL)prepare;
- (void)tick;
@end

@implementation ProxyProbe
- (void)emit:(NSDictionary *)record {
    NSError *error=nil;NSData *data=[NSJSONSerialization dataWithJSONObject:record options:NSJSONWritingSortedKeys error:&error];
    if (!data || fwrite(data.bytes,1,data.length,self.stream)!=data.length || fputc('\n',self.stream)==EOF || fflush(self.stream)!=0) {
        fprintf(stderr,"record retention failed: %s\n",error.description.UTF8String);exit(2);
    }
}
- (void)finish:(int)code reason:(NSString *)reason {
    if (self.stopped) return;self.stopped=YES;
    [self emit:@{@"kind":@"summary",@"exit_code":@(code),@"reason":reason,@"admitted_frames":@(self.admitted),
        @"completed_frames":@(self.completed),@"unresolved_frames":@(self.admitted-self.completed),@"skipped_admission_ticks":@(self.skipped),
        @"gpu_failed_frames":@(self.gpuFailures),@"pixel_failed_frames":@(self.pixelFailures),@"observation_unavailable_frames":@(self.unavailable),
        @"validated_for_governor":@NO}];
    fclose(self.stream);exit(code);
}
- (id<MTLTexture>)texture:(MTLPixelFormat)format width:(NSUInteger)width height:(NSUInteger)height usage:(MTLTextureUsage)usage {
    MTLTextureDescriptor *descriptor=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:format width:width height:height mipmapped:NO];
    descriptor.storageMode=MTLStorageModePrivate;descriptor.usage=usage;return [self.device newTextureWithDescriptor:descriptor];
}
- (MTLRenderPassDescriptor *)render:(id<MTLTexture>)texture clear:(double)value {
    MTLRenderPassDescriptor *descriptor=[MTLRenderPassDescriptor renderPassDescriptor];
    descriptor.colorAttachments[0].texture=texture;descriptor.colorAttachments[0].loadAction=MTLLoadActionClear;
    descriptor.colorAttachments[0].storeAction=MTLStoreActionStore;descriptor.colorAttachments[0].clearColor=MTLClearColorMake(value,value,value,value);
    return descriptor;
}
- (BOOL)clear:(id<MTLTexture>)texture value:(double)value buffer:(id<MTLCommandBuffer>)buffer label:(NSString *)label {
    id<MTLRenderCommandEncoder> encoder=[buffer renderCommandEncoderWithDescriptor:[self render:texture clear:value]];
    encoder.label=label;[encoder endEncoding];return encoder!=nil;
}
- (BOOL)prepare {
    self.device=MTLCreateSystemDefaultDevice();
    if (!self.device) { [self finish:2 reason:@"no Metal device"];return NO; }
    BOOL temporal=[self.options[@"--mode"] isEqual:@"temporal"];
    BOOL supported=temporal?[MTLFXTemporalScalerDescriptor supportsDevice:self.device]:[MTLFXSpatialScalerDescriptor supportsDevice:self.device];
    BOOL stageCounters=[self.device supportsCounterSampling:MTLCounterSamplingPointAtStageBoundary];
    for (id<MTLCounterSet> set in self.device.counterSets) if ([set.name isEqual:MTLCommonCounterSetTimestamp]) self.timestampSet=set;
    [self emit:@{@"kind":@"header",@"schema":@1,@"mode":self.options[@"--mode"],@"observe":self.options[@"--observe"],
        @"pid":@(getpid()),@"device":self.device.name,@"os":NSProcessInfo.processInfo.operatingSystemVersionString,
        @"target_frames":@(Frames),@"ring_size":@(Slots),@"input_width":@(InputWidth),@"input_height":@(InputHeight),
        @"output_width":@(OutputWidth),@"output_height":@(OutputHeight),@"scaler_supported":@(supported),
        @"stage_counters_supported":@(stageCounters),@"timestamp_counter_set":@(self.timestampSet!=nil),
        @"maximum_encoders_per_frame":@32,@"maximum_samples_per_frame":@128,@"maximum_selector_records_per_frame":@256,
        @"input_output_format":@"RGBA16Float",@"capture_format":@"RGBA8Unorm",@"input_pattern":@"red=x/160;green=y/90;blue=.2+frame*.02;alpha=1",
        @"temporal_history":@"one scaler; reset frame1 only; all frame inputs retained; zero jitter/motion; reversed depth0.5; exposure1",
        @"scope":@"supplied MetalFX command-buffer observation only; all-process encoder trace inventory still required",
        @"maximum_delivery_age_ms":@250,@"validated_for_governor":@NO,
        @"metal_debug_layer":NSProcessInfo.processInfo.environment[@"MTL_DEBUG_LAYER"]?:[NSNull null]}];
    if (!supported) { [self finish:2 reason:@"MetalFX scaler unsupported"];return NO; }
    if ([self.options[@"--observe"] isEqual:@"counters"] && (!stageCounters || !self.timestampSet)) {
        [self finish:2 reason:@"stage-boundary timestamps unavailable"];return NO;
    }
    if (temporal) {
        MTLFXTemporalScalerDescriptor *descriptor=[MTLFXTemporalScalerDescriptor new];
        descriptor.inputWidth=InputWidth;descriptor.inputHeight=InputHeight;descriptor.outputWidth=OutputWidth;descriptor.outputHeight=OutputHeight;
        descriptor.colorTextureFormat=MTLPixelFormatRGBA16Float;descriptor.outputTextureFormat=MTLPixelFormatRGBA16Float;
        descriptor.depthTextureFormat=MTLPixelFormatR32Float;descriptor.motionTextureFormat=MTLPixelFormatRG16Float;
        descriptor.autoExposureEnabled=NO;descriptor.requiresSynchronousInitialization=YES;descriptor.inputContentPropertiesEnabled=NO;
        self.temporal=[descriptor newTemporalScalerWithDevice:self.device];
    } else {
        MTLFXSpatialScalerDescriptor *descriptor=[MTLFXSpatialScalerDescriptor new];
        descriptor.inputWidth=InputWidth;descriptor.inputHeight=InputHeight;descriptor.outputWidth=OutputWidth;descriptor.outputHeight=OutputHeight;
        descriptor.colorTextureFormat=MTLPixelFormatRGBA16Float;descriptor.outputTextureFormat=MTLPixelFormatRGBA16Float;
        descriptor.colorProcessingMode=MTLFXSpatialScalerColorProcessingModeLinear;self.spatial=[descriptor newSpatialScalerWithDevice:self.device];
    }
    if (!self.spatial && !self.temporal) { [self finish:2 reason:@"MetalFX scaler creation failed"];return NO; }
    NSError *error=nil;id<MTLLibrary> library=[self.device newLibraryWithSource:Shaders options:nil error:&error];
    if (!library) { [self finish:2 reason:[NSString stringWithFormat:@"shader compilation: %@",error]];return NO; }
    for (NSString *name in @[@"input_color",@"composition"]) {
        MTLRenderPipelineDescriptor *descriptor=[MTLRenderPipelineDescriptor new];descriptor.vertexFunction=[library newFunctionWithName:@"vert"];
        descriptor.fragmentFunction=[library newFunctionWithName:name];descriptor.colorAttachments[0].pixelFormat=[name isEqual:@"input_color"]?MTLPixelFormatRGBA16Float:MTLPixelFormatRGBA8Unorm;
        id<MTLRenderPipelineState> pipeline=[self.device newRenderPipelineStateWithDescriptor:descriptor error:&error];
        if (!pipeline) { [self finish:2 reason:[NSString stringWithFormat:@"pipeline compilation: %@",error]];return NO; }
        if ([name isEqual:@"input_color"]) self.inputPipeline=pipeline;else self.composePipeline=pipeline;
    }
    self.queue=[self.device newCommandQueue];self.queue.label=@"metalfx-proxy-owned-queue";
    if (!self.queue) { [self finish:2 reason:@"queue creation failed"];return NO; }
    return YES;
}
- (NSDictionary *)pixels:(ProxyFrame *)frame {
    const uint8_t *png=frame.pngReadback.contents;const _Float16 *raw=frame.rawReadback.contents;
    NSUInteger alphaErrors=0,nonfiniteRGB=0;NSMutableSet *colors=[NSMutableSet new];
    double minimum=INFINITY,maximum=-INFINITY;
    for (NSUInteger i=0;i<OutputWidth*OutputHeight;i++) {
        alphaErrors+=png[i*4+3]!=255;
        if (i%16==0) [colors addObject:@((uint32_t)png[i*4]|((uint32_t)png[i*4+1]<<8)|((uint32_t)png[i*4+2]<<16))];
        for (NSUInteger c=0;c<3;c++) { double value=(double)raw[i*4+c];if (!isfinite(value)) nonfiniteRGB++;else { minimum=fmin(minimum,value);maximum=fmax(maximum,value); } }
    }
    return @{@"count":@(OutputWidth*OutputHeight),@"sentinel":@((uint32_t)png[0]|((uint32_t)png[1]<<8)),
        @"alpha_errors":@(alphaErrors),@"sampled_colors":@(colors.count),@"raw_nonfinite_rgb_values":@(nonfiniteRGB),
        @"raw_min_rgb":isfinite(minimum)?@(minimum):[NSNull null],@"raw_max_rgb":isfinite(maximum)?@(maximum):[NSNull null],
        @"metalfx_rgba16_sha256":Digest(frame.rawReadback.contents,OutputWidth*OutputHeight*8),
        @"composed_rgba8_sha256":Digest(frame.pngReadback.contents,OutputWidth*OutputHeight*4)};
}
- (BOOL)savePNG:(ProxyFrame *)frame {
    NSString *filename=[NSString stringWithFormat:@"frame-%04lu.png",(unsigned long)[frame.identity[@"frame"] unsignedIntegerValue]];
    NSURL *url=[NSURL fileURLWithPath:[self.directory stringByAppendingPathComponent:filename]];
    CGColorSpaceRef color=CGColorSpaceCreateDeviceRGB();
    CGContextRef context=CGBitmapContextCreate(frame.pngReadback.contents,OutputWidth,OutputHeight,8,OutputWidth*4,color,kCGBitmapByteOrder32Big|kCGImageAlphaPremultipliedLast);
    CGColorSpaceRelease(color);if (!context) return NO;
    CGImageRef image=CGBitmapContextCreateImage(context);
    CGImageDestinationRef destination=CGImageDestinationCreateWithURL((__bridge CFURLRef)url,CFSTR("public.png"),1,NULL);
    BOOL success=NO;if (image && destination) { CGImageDestinationAddImage(destination,image,NULL);success=CGImageDestinationFinalize(destination); }
    if (destination) CFRelease(destination);if (image) CGImageRelease(image);CGContextRelease(context);return success;
}
- (void)deliver:(ProxyFrame *)frame {
    if (self.stopped || frame.delivered || !frame.observation || !frame.pixelResult) return;
    frame.delivered=YES;
    BOOL pngSaved=frame.readbackSucceeded?[self savePNG:frame]:NO;
    NSString *rawPath=[self.directory stringByAppendingPathComponent:[NSString stringWithFormat:@"frame-%04lu.rgba16",(unsigned long)[frame.identity[@"frame"] unsignedIntegerValue]]];
    BOOL rawSaved=frame.readbackSucceeded && [[NSData dataWithBytes:frame.rawReadback.contents length:OutputWidth*OutputHeight*8] writeToFile:rawPath options:NSDataWritingWithoutOverwriting error:NULL];
    BOOL gpu=frame.setupSucceeded && frame.fxSucceeded && frame.readbackSucceeded;
    NSDictionary *pixels=frame.pixelResult;
    BOOL pixelOK=gpu && pngSaved && rawSaved && [pixels[@"sentinel"] isEqual:frame.identity[@"frame"]] && [pixels[@"alpha_errors"] unsignedIntegerValue]==0 &&
        [pixels[@"raw_nonfinite_rgb_values"] unsignedIntegerValue]==0 && [pixels[@"sampled_colors"] unsignedIntegerValue]>=16;
    BOOL available=[self.options[@"--observe"] isEqual:@"off"] || [frame.observation[@"available"] boolValue];
    uint64_t delivered=NowNS();
    if (delivered-frame.admittedNS>250*NSEC_PER_MSEC) available=NO;
    if (!gpu) self.gpuFailures++;if (!pixelOK) self.pixelFailures++;if (!available) self.unavailable++;
    [self emit:@{@"kind":@"completed",@"identity":frame.identity,@"setup_succeeded":@(frame.setupSucceeded),@"metalfx_succeeded":@(frame.fxSucceeded),
        @"readback_succeeded":@(frame.readbackSucceeded),@"owner_committed_metalfx":@(frame.ownerCommittedFx),@"png_saved":@(pngSaved),
        @"raw_saved":@(rawSaved),@"command_buffers":@{@"setup":frame.setupState,@"metalfx":frame.fxState,@"finish":frame.finishState},
        @"pixels":pixels,@"observation":frame.observation,@"setup_encoder_families":frame.setupFamilies,
        @"encode_start_host_ns":@(frame.encodeStartNS),@"encode_end_host_ns":@(frame.encodeEndNS),
        @"metalfx_callback_host_ns":@(frame.fxCallbackNS),@"counter_resolved_host_ns":@(frame.resolvedNS),
        @"readback_callback_host_ns":@(frame.readbackCallbackNS),@"delivered_host_ns":@(delivered),
        @"within_delivery_age_limit":@(delivered-frame.admittedNS<=250*NSEC_PER_MSEC),@"validated_for_governor":@NO}];
    self.slots[[frame.identity[@"slot"] unsignedIntegerValue]]=[NSNull null];self.completed++;[self tick];
}
- (void)launch:(NSUInteger)slot {
    NSUInteger frameID=self.admitted+1,generation=[self.generations[slot] unsignedIntegerValue]+1;
    ProxyFrame *frame=[ProxyFrame new];frame.identity=@{@"frame":@(frameID),@"view":@1,@"epoch":@1,@"slot":@(slot),@"generation":@(generation),
        @"mode":self.options[@"--mode"],@"observe":self.options[@"--observe"],@"input_width":@(InputWidth),@"input_height":@(InputHeight),
        @"output_width":@(OutputWidth),@"output_height":@(OutputHeight),@"reset":@(self.temporal && frameID==1)};
    frame.prefix=[NSString stringWithFormat:@"proxy/frame=%lu/view=1/epoch=1/slot=%lu/gen=%lu",(unsigned long)frameID,(unsigned long)slot,(unsigned long)generation];
    MTLTextureUsage inputUsage=self.temporal?self.temporal.colorTextureUsage:self.spatial.colorTextureUsage;
    MTLTextureUsage outputUsage=self.temporal?self.temporal.outputTextureUsage:self.spatial.outputTextureUsage;
    frame.input=[self texture:MTLPixelFormatRGBA16Float width:InputWidth height:InputHeight usage:inputUsage|MTLTextureUsageRenderTarget];
    frame.scaled=[self texture:MTLPixelFormatRGBA16Float width:OutputWidth height:OutputHeight usage:outputUsage|MTLTextureUsageShaderRead|MTLTextureUsageRenderTarget];
    frame.composed=[self texture:MTLPixelFormatRGBA8Unorm width:OutputWidth height:OutputHeight usage:MTLTextureUsageRenderTarget];
    frame.rawReadback=[self.device newBufferWithLength:OutputWidth*OutputHeight*8 options:MTLResourceStorageModeShared];
    frame.pngReadback=[self.device newBufferWithLength:OutputWidth*OutputHeight*4 options:MTLResourceStorageModeShared];
    if (self.temporal) {
        frame.depth=[self texture:MTLPixelFormatR32Float width:InputWidth height:InputHeight usage:self.temporal.depthTextureUsage|MTLTextureUsageRenderTarget];
        frame.motion=[self texture:MTLPixelFormatRG16Float width:InputWidth height:InputHeight usage:self.temporal.motionTextureUsage|MTLTextureUsageRenderTarget];
        frame.exposure=[self texture:MTLPixelFormatR16Float width:1 height:1 usage:MTLTextureUsageShaderRead|MTLTextureUsageRenderTarget];
    }
    if (!frame.input || !frame.scaled || !frame.composed || !frame.rawReadback || !frame.pngReadback ||
        (self.temporal && (!frame.depth || !frame.motion || !frame.exposure))) { [self finish:2 reason:@"frame resource allocation failed"];return; }
    id<MTLCommandBuffer> setup=[self.queue commandBuffer],fx=[self.queue commandBuffer],finish=[self.queue commandBuffer];
    setup.label=[frame.prefix stringByAppendingString:@"/setup"];fx.label=[frame.prefix stringByAppendingString:@"/metalfx"];finish.label=[frame.prefix stringByAppendingString:@"/finish"];
    if (!setup || !fx || !finish) { [self finish:2 reason:@"command buffer allocation failed"];return; }
    NSMutableArray *families=[NSMutableArray arrayWithArray:@[@"scene-input",@"output-clear"]];
    id<MTLRenderCommandEncoder> input=[setup renderCommandEncoderWithDescriptor:[self render:frame.input clear:0]];
    input.label=[frame.prefix stringByAppendingString:@"/scene-input"];[input setRenderPipelineState:self.inputPipeline];
    uint32_t parameter=(uint32_t)frameID;[input setFragmentBytes:&parameter length:sizeof(parameter) atIndex:0];
    [input drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];[input endEncoding];
    BOOL encoded=input!=nil;
    encoded&=[self clear:frame.scaled value:0 buffer:setup label:[frame.prefix stringByAppendingString:@"/output-clear"]];
    if (self.temporal) {
        [families addObjectsFromArray:@[@"depth-input",@"motion-input",@"exposure-input"]];
        encoded&=[self clear:frame.depth value:.5 buffer:setup label:[frame.prefix stringByAppendingString:@"/depth-input"]];
        encoded&=[self clear:frame.motion value:0 buffer:setup label:[frame.prefix stringByAppendingString:@"/motion-input"]];
        encoded&=[self clear:frame.exposure value:1 buffer:setup label:[frame.prefix stringByAppendingString:@"/exposure-input"]];
    }
    frame.setupFamilies=[families copy];
    id<MTLRenderCommandEncoder> compose=[finish renderCommandEncoderWithDescriptor:[self render:frame.composed clear:0]];
    compose.label=[frame.prefix stringByAppendingString:@"/compose"];[compose setRenderPipelineState:self.composePipeline];
    [compose setFragmentTexture:frame.scaled atIndex:0];[compose setFragmentBytes:&parameter length:sizeof(parameter) atIndex:0];
    [compose drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];[compose endEncoding];
    id<MTLBlitCommandEncoder> readback=[finish blitCommandEncoder];readback.label=[frame.prefix stringByAppendingString:@"/readback"];
    [readback copyFromTexture:frame.scaled sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0) sourceSize:MTLSizeMake(OutputWidth,OutputHeight,1) toBuffer:frame.rawReadback destinationOffset:0 destinationBytesPerRow:OutputWidth*8 destinationBytesPerImage:OutputWidth*OutputHeight*8];
    [readback copyFromTexture:frame.composed sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0) sourceSize:MTLSizeMake(OutputWidth,OutputHeight,1) toBuffer:frame.pngReadback destinationOffset:0 destinationBytesPerRow:OutputWidth*4 destinationBytesPerImage:OutputWidth*OutputHeight*4];[readback endEncoding];
    if (!encoded || !compose || !readback) { [self finish:2 reason:@"setup/composition encoder allocation failed"];return; }
    BOOL observing=![self.options[@"--observe"] isEqual:@"off"];
    if (observing) {
        id<MTLDevice> counterDevice=self.device;id<MTLCounterSet> counterSet=self.timestampSet;
        frame.ledger=[[ObservationLedger alloc] initWithIdentity:frame.identity expectedLabel:fx.label
            mode:[self.options[@"--observe"] isEqual:@"counters"]?ObservationModeCounters:ObservationModeCalls maximumEncoders:32
            counterFactory:^id<MTLCounterSampleBuffer>(NSString *label,NSUInteger count,NSError **error) {
                MTLCounterSampleBufferDescriptor *descriptor=[MTLCounterSampleBufferDescriptor new];descriptor.counterSet=counterSet;
                descriptor.storageMode=MTLStorageModeShared;descriptor.sampleCount=count;descriptor.label=label;
                return [counterDevice newCounterSampleBufferWithDescriptor:descriptor error:error];
            }];
    }
    [self.retainedFrames addObject:frame];self.slots[slot]=frame;self.generations[slot]=@(generation);self.admitted++;
    frame.admittedNS=NowNS();[self emit:@{@"kind":@"admitted",@"identity":frame.identity,@"host_ns":@(frame.admittedNS),@"command_buffer_prefix":frame.prefix}];
    [fx addCompletedHandler:^(id<MTLCommandBuffer> buffer) {
        @autoreleasepool {
            uint64_t callback=NowNS();BOOL success=buffer.status==MTLCommandBufferStatusCompleted && !buffer.error;
            NSDictionary *state=BufferState(buffer);
            NSDictionary *observation=frame.ledger?[frame.ledger completeCommandBuffer:buffer]:@{@"observation_mode":@"off",@"available":@NO,@"not_requested":@YES,@"validated_for_governor":@NO};
            uint64_t resolved=NowNS();
            dispatch_async(self.control, ^{ frame.fxCallbackNS=callback;frame.resolvedNS=resolved;frame.fxSucceeded=success;frame.fxState=state;frame.observation=observation;[self deliver:frame]; });
        }
    }];
    [finish addCompletedHandler:^(id<MTLCommandBuffer> buffer) {
        @autoreleasepool {
            uint64_t callback=NowNS();BOOL success=buffer.status==MTLCommandBufferStatusCompleted && !buffer.error;
            BOOL sourceSucceeded=setup.status==MTLCommandBufferStatusCompleted && !setup.error;
            NSDictionary *setupState=BufferState(setup),*finishState=BufferState(buffer);
            NSDictionary *pixels=success?[self pixels:frame]:@{};
            dispatch_async(self.control, ^{ frame.setupSucceeded=sourceSucceeded;frame.readbackSucceeded=success;frame.setupState=setupState;frame.finishState=finishState;frame.readbackCallbackNS=callback;frame.pixelResult=pixels;[self deliver:frame]; });
        }
    }];
    [setup commit];
    id<MTLCommandBuffer> observed=observing?[ObservedCommandBuffer wrap:fx ledger:frame.ledger]:fx;
    frame.encodeStartNS=NowNS();
    @try {
        if (self.temporal) {
            self.temporal.colorTexture=frame.input;self.temporal.depthTexture=frame.depth;self.temporal.motionTexture=frame.motion;
            self.temporal.exposureTexture=frame.exposure;self.temporal.outputTexture=frame.scaled;self.temporal.preExposure=1;
            self.temporal.inputContentWidth=InputWidth;self.temporal.inputContentHeight=InputHeight;self.temporal.depthReversed=YES;
            self.temporal.jitterOffsetX=0;self.temporal.jitterOffsetY=0;self.temporal.motionVectorScaleX=1;self.temporal.motionVectorScaleY=1;self.temporal.reset=frameID==1;
            [self.temporal encodeToCommandBuffer:observed];
        } else {
            self.spatial.colorTexture=frame.input;self.spatial.outputTexture=frame.scaled;
            self.spatial.inputContentWidth=InputWidth;self.spatial.inputContentHeight=InputHeight;[self.spatial encodeToCommandBuffer:observed];
        }
    } @catch (NSException *exception) {
        [self emit:@{@"kind":@"encode_exception",@"identity":frame.identity,@"name":exception.name,@"reason":exception.reason?:@"unknown"}];
        [self finish:2 reason:@"MetalFX encode raised an exception; partial records retained"];return;
    }
    frame.encodeEndNS=NowNS();[frame.ledger sealCommandBuffer:fx];
    if (!observing && (fx.status!=MTLCommandBufferStatusNotEnqueued || ![fx.label isEqual:[frame.prefix stringByAppendingString:@"/metalfx"]])) {
        [self emit:@{@"kind":@"unexpected_unproxied_submission_or_label",@"identity":frame.identity,@"actual_status":@(fx.status),@"actual_label":fx.label?:[NSNull null]}];
        self.unavailable++;
    }
    frame.ownerCommittedFx=ObservationCommitIfNeeded(fx);[finish commit];
}
- (void)tick {
    @autoreleasepool {
        if (self.stopped) return;
        if (self.completed==Frames) { [self finish:(self.gpuFailures || self.pixelFailures || self.unavailable)?1:0 reason:@"all frame callbacks and readbacks retained"];return; }
        if (NowNS()-self.startedNS>15*NSEC_PER_SEC) { [self finish:2 reason:@"bounded deadline; unresolved frames unavailable"];return; }
        if (self.admitted==Frames) return;
        NSUInteger slot=[self.slots indexOfObject:[NSNull null]];
        if (slot==NSNotFound) { self.skipped++;return; }[self launch:slot];
    }
}
@end

int main(int argc,const char *argv[]) {
    @autoreleasepool {
        if (argc==2 && strcmp(argv[1],"--self-test")==0) {
            const char *good[]={"probe","--mode","spatial","--observe","calls","--out","/tmp/fresh"};
            const char *bad[]={"probe","--mode","spatial","--mode","calls","--out","/tmp/fresh"};
            if (!Options(7,good) || Options(6,good) || Options(7,bad)) return 1;
            printf("3 CLI checks passed; no Metal device created\n");return 0;
        }
        NSDictionary *options=Options(argc,argv);
        if (!options) { fprintf(stderr,"usage: metalfx-proxy --mode spatial|temporal --observe off|calls|counters --out NEW_DIRECTORY\n");return 2; }
        ProxyProbe *probe=[ProxyProbe new];probe.options=options;probe.directory=options[@"--out"];
        NSError *error=nil;
        if ([[NSFileManager defaultManager] fileExistsAtPath:probe.directory] || ![[NSFileManager defaultManager] createDirectoryAtPath:probe.directory withIntermediateDirectories:YES attributes:nil error:&error]) {
            fprintf(stderr,"fresh output directory required\n");return 2;
        }
        int fd=open([[probe.directory stringByAppendingPathComponent:@"samples.jsonl"] fileSystemRepresentation],O_WRONLY|O_CREAT|O_EXCL,0600);
        if (fd<0) return 2;probe.stream=fdopen(fd,"w");if (!probe.stream) { close(fd);return 2; }
        if (![probe prepare]) return 2;
        probe.control=dispatch_queue_create("metalfx-proxy-control",DISPATCH_QUEUE_SERIAL);
        probe.slots=[NSMutableArray new];probe.generations=[NSMutableArray new];probe.retainedFrames=[NSMutableArray new];
        for (NSUInteger i=0;i<Slots;i++) { [probe.slots addObject:[NSNull null]];[probe.generations addObject:@0]; }
        probe.startedNS=NowNS();probe.timer=dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER,0,0,probe.control);
        dispatch_source_set_timer(probe.timer,DISPATCH_TIME_NOW,NSEC_PER_MSEC,NSEC_PER_MSEC/10);
        dispatch_source_set_event_handler(probe.timer, ^{ [probe tick]; });dispatch_resume(probe.timer);dispatch_main();
    }
}
