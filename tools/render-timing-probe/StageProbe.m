// Isolated actual-pass counter experiment. Never publishes a validated governor signal.
// Root controls compilation and GPU execution. See docs/research/gpu-producer-02.md.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <CoreGraphics/CoreGraphics.h>
#import <ImageIO/ImageIO.h>
#import <mach/mach_time.h>
#include <fcntl.h>
#include <unistd.h>

static const NSUInteger Width = 320, Height = 180, TargetFrames = 32, RingSize = 4;
static uint64_t HostNS(void) {
    static mach_timebase_info_data_t info;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ mach_timebase_info(&info); });
    return (uint64_t)(((__uint128_t)mach_absolute_time() * info.numer) / info.denom);
}

static const char *ShaderSource =
"#include <metal_stdlib>\n"
"using namespace metal;\n"
"struct VertexOut { float4 position [[position]]; };\n"
"struct Params { uint frame; uint iterations; };\n"
"vertex VertexOut vertex_main(uint id [[vertex_id]]) {\n"
"    const float2 p[3] = {float2(-1,-1),float2(3,-1),float2(-1,3)};\n"
"    return {float4(p[id],0,1)};\n"
"}\n"
"fragment float4 scene_fragment(VertexOut v [[stage_in]], constant Params &p [[buffer(0)]]) {\n"
"    uint2 xy = uint2(v.position.xy);\n"
"    if (all(xy == uint2(0))) return float4(float(p.frame & 255)/255.0f,float((p.frame>>8)&255)/255.0f,0.5,1);\n"
"    float value = v.position.x * .0031f + v.position.y * .0047f;\n"
"    for (uint i=0;i<p.iterations;++i) value = sin(value * 1.001f + float(i) * .00001f);\n"
"    return float4(fract(v.position.x/320.0f),fract(v.position.y/180.0f),value*.4f+.5f,1);\n"
"}\n"
"kernel void dependent_compute(texture2d<float,access::read> source [[texture(0)]],\n"
"                              texture2d<float,access::write> target [[texture(1)]],\n"
"                              uint2 xy [[thread_position_in_grid]]) {\n"
"    float4 color = source.read(xy);\n"
"    if (any(xy != uint2(0))) color.b = 1.0f-color.b;\n"
"    target.write(color,xy);\n"
"}\n"
"fragment float4 compose_fragment(VertexOut v [[stage_in]], texture2d<float,access::read> source [[texture(0)]]) {\n"
"    return source.read(uint2(v.position.xy));\n"
"}\n";

@interface Frame : NSObject
@property(nonatomic,strong) NSDictionary *identity;
@property(nonatomic,strong) id<MTLTexture> scene;
@property(nonatomic,strong) id<MTLTexture> intermediate;
@property(nonatomic,strong) id<MTLTexture> output;
@property(nonatomic,strong) id<MTLBuffer> readback;
@property(nonatomic,strong) NSDictionary<NSString *,id<MTLCounterSampleBuffer>> *counters;
@property(nonatomic) uint64_t hostNS;
@property(nonatomic) uint64_t firstSubmitNS;
@property(nonatomic) uint64_t secondSubmitNS;
@end
@implementation Frame
@end

@interface Probe : NSObject
@property(nonatomic,strong) id<MTLDevice> device;
@property(nonatomic,strong) id<MTLCommandQueue> queue;
@property(nonatomic,strong) id<MTLCounterSet> timestampSet;
@property(nonatomic,strong) id<MTLRenderPipelineState> scenePipeline;
@property(nonatomic,strong) id<MTLRenderPipelineState> composePipeline;
@property(nonatomic,strong) id<MTLComputePipelineState> computePipeline;
@property(nonatomic,strong) dispatch_queue_t control;
@property(nonatomic,strong) dispatch_source_t timer;
@property(nonatomic,copy) NSString *directory;
@property(nonatomic) FILE *stream;
@property(nonatomic,strong) NSMutableArray *slots;
@property(nonatomic,strong) NSMutableArray<NSNumber *> *generations;
@property(nonatomic) NSUInteger admitted;
@property(nonatomic) NSUInteger completed;
@property(nonatomic) NSUInteger skipped;
@property(nonatomic) NSUInteger commandErrors;
@property(nonatomic) uint64_t startedNS;
@property(nonatomic) BOOL stopped;
- (void)emit:(NSDictionary *)record;
- (void)tick;
@end

@implementation Probe
- (void)emit:(NSDictionary *)record {
    NSError *error = nil;
    NSData *data = [NSJSONSerialization dataWithJSONObject:record options:NSJSONWritingSortedKeys error:&error];
    if (!data || fwrite(data.bytes,1,data.length,self.stream)!=data.length || fputc('\n',self.stream)==EOF || fflush(self.stream)!=0) {
        fprintf(stderr,"failed to retain diagnostic record: %s\n",error.description.UTF8String);
        exit(2);
    }
}
- (void)finish:(int)code reason:(NSString *)reason {
    if (self.stopped) return;
    self.stopped = YES;
    [self emit:@{@"kind":@"summary",@"exit_code":@(code),@"reason":reason,
        @"admitted_frames":@(self.admitted),@"completed_frames":@(self.completed),
        @"unresolved_frames":@(self.admitted-self.completed),@"skipped_admission_ticks":@(self.skipped),
        @"command_errors":@(self.commandErrors),@"validated_for_governor":@NO}];
    fclose(self.stream);
    // This bounded native CLI exits normally only after every admitted buffer completes.
    // A deadline exits nonzero with unresolved identities retained; never a timing success.
    exit(code);
}
- (id<MTLTexture>)texture:(MTLTextureUsage)usage {
    MTLTextureDescriptor *d=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:Width height:Height mipmapped:NO];
    d.storageMode=MTLStorageModePrivate;
    d.usage=usage;
    return [self.device newTextureWithDescriptor:d];
}
- (id<MTLCounterSampleBuffer>)counter:(NSString *)name count:(NSUInteger)count error:(NSError **)error {
    MTLCounterSampleBufferDescriptor *d=[MTLCounterSampleBufferDescriptor new];
    d.counterSet=self.timestampSet;
    d.storageMode=MTLStorageModeShared;
    d.sampleCount=count;
    d.label=name;
    return [self.device newCounterSampleBufferWithDescriptor:d error:error];
}
- (MTLRenderPassDescriptor *)renderDescriptor:(id<MTLTexture>)texture counter:(id<MTLCounterSampleBuffer>)counter {
    MTLRenderPassDescriptor *d=[MTLRenderPassDescriptor renderPassDescriptor];
    d.colorAttachments[0].texture=texture;
    d.colorAttachments[0].loadAction=MTLLoadActionClear;
    d.colorAttachments[0].storeAction=MTLStoreActionStore;
    d.colorAttachments[0].clearColor=MTLClearColorMake(0,0,0,1);
    MTLRenderPassSampleBufferAttachmentDescriptor *a=d.sampleBufferAttachments[0];
    a.sampleBuffer=counter;
    a.startOfVertexSampleIndex=0; a.endOfVertexSampleIndex=1;
    a.startOfFragmentSampleIndex=2; a.endOfFragmentSampleIndex=3;
    return d;
}
- (NSArray *)resolve:(id<MTLCounterSampleBuffer>)counter count:(NSUInteger)count {
    NSData *data=[counter resolveCounterRange:NSMakeRange(0,count)];
    if (!data || data.length!=count*sizeof(MTLCounterResultTimestamp)) return @[];
    const MTLCounterResultTimestamp *values=data.bytes;
    NSMutableArray *result=[NSMutableArray arrayWithCapacity:count];
    for (NSUInteger i=0;i<count;++i) [result addObject:@(values[i].timestamp)];
    return result;
}
- (NSDictionary *)pixels:(id<MTLBuffer>)buffer {
    const uint8_t *b=buffer.contents;
    NSUInteger alphaErrors=0;
    NSMutableSet *colors=[NSMutableSet new];
    for (NSUInteger i=0;i<Width*Height;++i) {
        alphaErrors += b[i*4+3]!=255;
        if (i%16==0) [colors addObject:@((uint32_t)b[i*4] | ((uint32_t)b[i*4+1]<<8) | ((uint32_t)b[i*4+2]<<16))];
    }
    return @{@"sentinel":@((uint32_t)b[0] | ((uint32_t)b[1]<<8)),@"count":@(Width*Height),
             @"alpha_errors":@(alphaErrors),@"sampled_colors":@(colors.count)};
}
- (BOOL)savePNG:(Frame *)frame {
    NSString *name=[NSString stringWithFormat:@"frame-%04lu.png",(unsigned long)[frame.identity[@"frame"] unsignedIntegerValue]];
    NSURL *url=[NSURL fileURLWithPath:[self.directory stringByAppendingPathComponent:name]];
    CGColorSpaceRef color=CGColorSpaceCreateDeviceRGB();
    CGContextRef context=CGBitmapContextCreate(frame.readback.contents,Width,Height,8,Width*4,color,kCGBitmapByteOrder32Big|kCGImageAlphaPremultipliedLast);
    CGColorSpaceRelease(color);
    if (!context) return NO;
    CGImageRef image=CGBitmapContextCreateImage(context);
    CGImageDestinationRef destination=CGImageDestinationCreateWithURL((__bridge CFURLRef)url,CFSTR("public.png"),1,NULL);
    BOOL saved=NO;
    if (image && destination) { CGImageDestinationAddImage(destination,image,NULL); saved=CGImageDestinationFinalize(destination); }
    if (destination) CFRelease(destination);
    if (image) CGImageRelease(image);
    CGContextRelease(context);
    return saved;
}
- (void)launch:(NSUInteger)slot {
    NSUInteger frameID=self.admitted+1, arm=(frameID-1)%4;
    NSUInteger iterations=(arm==1 || arm==3)?1000:0, gap=arm>=2?20:0;
    NSUInteger generation=[self.generations[slot] unsignedIntegerValue]+1;
    Frame *frame=[Frame new];
    frame.identity=@{@"frame":@(frameID),@"view":@1,@"epoch":@(arm+1),@"slot":@(slot),
        @"generation":@(generation),@"width":@(Width),@"height":@(Height),
        @"iterations":@(iterations),@"cpu_gap_ms":@(gap)};
    frame.scene=[self texture:MTLTextureUsageRenderTarget|MTLTextureUsageShaderRead];
    frame.intermediate=[self texture:MTLTextureUsageShaderRead|MTLTextureUsageShaderWrite];
    frame.output=[self texture:MTLTextureUsageRenderTarget];
    frame.readback=[self.device newBufferWithLength:Width*Height*4 options:MTLResourceStorageModeShared];
    NSError *error=nil;
    NSMutableDictionary *counters=[NSMutableDictionary new];
    for (NSString *name in @[@"scene",@"compute",@"compose",@"readback"]) {
        NSUInteger count=([name isEqualToString:@"scene"]||[name isEqualToString:@"compose"])?4:2;
        id<MTLCounterSampleBuffer> counter=[self counter:[NSString stringWithFormat:@"frame%lu-%@",(unsigned long)frameID,name] count:count error:&error];
        if (!counter) { [self finish:2 reason:[NSString stringWithFormat:@"counter allocation: %@",error]]; return; }
        counters[name]=counter;
    }
    frame.counters=counters;
    if (!frame.scene || !frame.intermediate || !frame.output || !frame.readback) { [self finish:2 reason:@"resource allocation failed"]; return; }
    id<MTLCommandBuffer> first=[self.queue commandBuffer], second=[self.queue commandBuffer];
    NSString *label=[NSString stringWithFormat:@"stage-probe/frame=%lu/view=1/epoch=%lu/slot=%lu/gen=%lu",(unsigned long)frameID,(unsigned long)(arm+1),(unsigned long)slot,(unsigned long)generation];
    first.label=[label stringByAppendingString:@"/scene"];
    second.label=[label stringByAppendingString:@"/compute-compose-readback"];
    id<MTLRenderCommandEncoder> scene=[first renderCommandEncoderWithDescriptor:[self renderDescriptor:frame.scene counter:counters[@"scene"]]];
    scene.label=first.label;
    [scene setRenderPipelineState:self.scenePipeline];
    const uint32_t params[2]={(uint32_t)frameID,(uint32_t)iterations};
    [scene setFragmentBytes:params length:sizeof(params) atIndex:0];
    [scene drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
    [scene endEncoding];
    MTLComputePassDescriptor *computeDesc=[MTLComputePassDescriptor computePassDescriptor];
    computeDesc.sampleBufferAttachments[0].sampleBuffer=counters[@"compute"];
    computeDesc.sampleBufferAttachments[0].startOfEncoderSampleIndex=0;
    computeDesc.sampleBufferAttachments[0].endOfEncoderSampleIndex=1;
    id<MTLComputeCommandEncoder> compute=[second computeCommandEncoderWithDescriptor:computeDesc];
    compute.label=[label stringByAppendingString:@"/compute"];
    [compute setComputePipelineState:self.computePipeline];
    [compute setTexture:frame.scene atIndex:0]; [compute setTexture:frame.intermediate atIndex:1];
    [compute dispatchThreads:MTLSizeMake(Width,Height,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
    [compute endEncoding];
    id<MTLRenderCommandEncoder> compose=[second renderCommandEncoderWithDescriptor:[self renderDescriptor:frame.output counter:counters[@"compose"]]];
    compose.label=[label stringByAppendingString:@"/compose"];
    [compose setRenderPipelineState:self.composePipeline];
    [compose setFragmentTexture:frame.intermediate atIndex:0];
    [compose drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
    [compose endEncoding];
    MTLBlitPassDescriptor *blitDesc=[MTLBlitPassDescriptor blitPassDescriptor];
    blitDesc.sampleBufferAttachments[0].sampleBuffer=counters[@"readback"];
    blitDesc.sampleBufferAttachments[0].startOfEncoderSampleIndex=0;
    blitDesc.sampleBufferAttachments[0].endOfEncoderSampleIndex=1;
    id<MTLBlitCommandEncoder> blit=[second blitCommandEncoderWithDescriptor:blitDesc];
    blit.label=[label stringByAppendingString:@"/diagnostic-readback"];
    [blit copyFromTexture:frame.output sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0) sourceSize:MTLSizeMake(Width,Height,1) toBuffer:frame.readback destinationOffset:0 destinationBytesPerRow:Width*4 destinationBytesPerImage:Width*Height*4];
    [blit endEncoding];
    if (!first || !second || !scene || !compute || !compose || !blit) { [self finish:2 reason:@"encoder allocation failed"]; return; }
    self.slots[slot]=frame;
    self.generations[slot]=@(generation);
    self.admitted++;
    frame.hostNS=HostNS();
    [self emit:@{@"kind":@"admitted",@"identity":frame.identity,@"host_ns":@(frame.hostNS)}];
    [second addCompletedHandler:^(id<MTLCommandBuffer> buffer) {
        @autoreleasepool {
            uint64_t callback=HostNS();
            BOOL success=buffer.status==MTLCommandBufferStatusCompleted && first.status==MTLCommandBufferStatusCompleted;
            NSMutableDictionary *passes=[NSMutableDictionary new];
            for (NSString *name in @[@"scene",@"compute",@"compose",@"readback"]) {
                NSUInteger count=([name isEqualToString:@"scene"]||[name isEqualToString:@"compose"])?4:2;
                passes[name]=success?[self resolve:frame.counters[name] count:count]:@[];
            }
            uint64_t resolved=HostNS();
            NSDictionary *pixels=success?[self pixels:frame.readback]:@{};
            NSString *gpuError=[NSString stringWithFormat:@"first=%@; second=%@",first.error,buffer.error];
            dispatch_async(self.control, ^{
                if (self.stopped) return;
                BOOL pngSaved=YES;
                if (success && frameID>TargetFrames-4) pngSaved=[self savePNG:frame];
                [self emit:@{@"kind":@"completed",@"identity":frame.identity,@"status":success?@"completed":@"error",
                    @"callback_host_ns":@(callback),@"resolved_host_ns":@(resolved),@"delivered_host_ns":@(HostNS()),
                    @"first_submit_host_ns":@(frame.firstSubmitNS),@"second_submit_host_ns":@(frame.secondSubmitNS),
                    @"passes":passes,@"pixels":pixels,@"gpu_error":gpuError,@"selected_png_saved":@(pngSaved)}];
                if (!success || !pngSaved) self.commandErrors++;
                self.slots[slot]=[NSNull null];
                self.completed++;
                [self tick];
            });
        }
    }];
    frame.firstSubmitNS=HostNS();
    [first commit];
    // Delay submission asynchronously. The control/render loop never sleeps or waits for a GPU fence.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW,(int64_t)(gap*NSEC_PER_MSEC)),self.control, ^{
        if (!self.stopped) { frame.secondSubmitNS=HostNS(); [second commit]; }
    });
}
- (void)tick {
    @autoreleasepool {
        if (self.stopped) return;
        if (self.completed==TargetFrames) { [self finish:self.commandErrors?1:0 reason:@"all admitted callbacks delivered"]; return; }
        if (HostNS()-self.startedNS>15*NSEC_PER_SEC) { [self finish:2 reason:@"bounded probe deadline; unresolved records are failures"]; return; }
        if (self.admitted>=TargetFrames) return;
        NSUInteger slot=[self.slots indexOfObject:[NSNull null]];
        if (slot==NSNotFound) { self.skipped++; return; }
        [self launch:slot];
    }
}
@end

int main(int argc,const char *argv[]) {
    @autoreleasepool {
        if (argc!=3 || strcmp(argv[1],"--out")!=0) { fprintf(stderr,"usage: stage-probe --out NEW_DIRECTORY\n"); return 2; }
        Probe *probe=[Probe new];
        probe.directory=[NSString stringWithUTF8String:argv[2]];
        NSError *error=nil;
        if ([[NSFileManager defaultManager] fileExistsAtPath:probe.directory] || ![[NSFileManager defaultManager] createDirectoryAtPath:probe.directory withIntermediateDirectories:YES attributes:nil error:&error]) { fprintf(stderr,"fresh output directory required\n"); return 2; }
        NSString *log=[probe.directory stringByAppendingPathComponent:@"samples.jsonl"];
        int fd=open(log.fileSystemRepresentation,O_WRONLY|O_CREAT|O_EXCL,0600);
        if (fd<0) return 2;
        probe.stream=fdopen(fd,"w");
        if (!probe.stream) { close(fd); return 2; }
        probe.device=MTLCreateSystemDefaultDevice();
        if (!probe.device || ![probe.device supportsFamily:MTLGPUFamilyApple1] || ![probe.device supportsCounterSampling:MTLCounterSamplingPointAtStageBoundary]) { [probe finish:2 reason:@"Apple GPU stage-boundary counters unavailable"]; return 2; }
        for (id<MTLCounterSet> set in probe.device.counterSets) if ([set.name isEqualToString:MTLCommonCounterSetTimestamp]) probe.timestampSet=set;
        if (!probe.timestampSet) { [probe finish:2 reason:@"timestamp counter set unavailable"]; return 2; }
        probe.queue=[probe.device newCommandQueue];
        probe.queue.label=@"stage-probe-owned-queue";
        id<MTLLibrary> library=[probe.device newLibraryWithSource:[NSString stringWithUTF8String:ShaderSource] options:nil error:&error];
        if (!library) { [probe finish:2 reason:[NSString stringWithFormat:@"shader compilation: %@",error]]; return 2; }
        for (NSString *fragment in @[@"scene_fragment",@"compose_fragment"]) {
            MTLRenderPipelineDescriptor *d=[MTLRenderPipelineDescriptor new];
            d.vertexFunction=[library newFunctionWithName:@"vertex_main"];
            d.fragmentFunction=[library newFunctionWithName:fragment];
            d.colorAttachments[0].pixelFormat=MTLPixelFormatRGBA8Unorm;
            id<MTLRenderPipelineState> pipeline=[probe.device newRenderPipelineStateWithDescriptor:d error:&error];
            if (!pipeline) { [probe finish:2 reason:[NSString stringWithFormat:@"pipeline compilation: %@",error]]; return 2; }
            if ([fragment isEqualToString:@"scene_fragment"]) probe.scenePipeline=pipeline; else probe.composePipeline=pipeline;
        }
        probe.computePipeline=[probe.device newComputePipelineStateWithFunction:[library newFunctionWithName:@"dependent_compute"] error:&error];
        if (!probe.computePipeline || !probe.queue) { [probe finish:2 reason:@"compute pipeline or queue creation failed"]; return 2; }
        probe.control=dispatch_queue_create("stage-probe-control",DISPATCH_QUEUE_SERIAL);
        probe.slots=[NSMutableArray new]; probe.generations=[NSMutableArray new];
        for (NSUInteger i=0;i<RingSize;++i) { [probe.slots addObject:[NSNull null]]; [probe.generations addObject:@0]; }
        probe.startedNS=HostNS();
        [probe emit:@{@"kind":@"header",@"schema":@1,@"device":probe.device.name,@"pid":@(getpid()),
            @"os":NSProcessInfo.processInfo.operatingSystemVersionString,@"counter_mode":@"four render stage boundaries; compute/blit encoder boundaries",
            @"resolve_mode":@"CPU resolveCounterRange in final command-buffer completion callback",
            @"target_frames":@(TargetFrames),@"ring_size":@(RingSize),@"counter_buffers_per_frame":@4,
            @"width":@(Width),@"height":@(Height),@"maximum_delivery_age_ms":@250,
            @"gpu_clock":@"MTLCounterResultTimestamp nanoseconds on Apple GPU; no CPU/GPU subtraction",
            @"scope":@"synthetic scene render, dependent compute, composition render; diagnostic readback blit reported separately",
            @"contains_metalfx":@NO,@"validated_for_governor":@NO,
            @"metal_debug_layer":NSProcessInfo.processInfo.environment[@"MTL_DEBUG_LAYER"]?:[NSNull null]}];
        probe.timer=dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER,0,0,probe.control);
        dispatch_source_set_timer(probe.timer,DISPATCH_TIME_NOW,NSEC_PER_MSEC,NSEC_PER_MSEC/10);
        dispatch_source_set_event_handler(probe.timer, ^{ [probe tick]; });
        dispatch_resume(probe.timer);
        dispatch_main();
    }
}
