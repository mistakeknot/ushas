#import <Foundation/Foundation.h>
#import <Metal/Metal.h>

NS_ASSUME_NONNULL_BEGIN

typedef NS_ENUM(NSUInteger, ObservationMode) {
    ObservationModeCalls,
    ObservationModeCounters,
};
typedef id<MTLCounterSampleBuffer> _Nullable (^ObservationCounterFactory)(NSString *label, NSUInteger count, NSError * _Nullable * _Nullable error);

// Internal to the isolated probe. No Ushas or general-purpose API is introduced.
// USHAS_OBSERVATION_CAPTURE_UNKNOWN_STACK=1 is latched at ledger creation.
// It captures at most four unknown-invocation stacks, each at most 32 frames,
// before forwarding. This synchronous CPU diagnostic is attribution evidence,
// not timing evidence. Unknown selectors still invalidate the observation.
// Missing or overlong (>1024 UTF-8 bytes) image/symbol/class metadata is null.
// Captured PCs, load addresses and offsets remain hexadecimal strings.
@interface ObservationLedger : NSObject
- (instancetype)initWithIdentity:(NSDictionary *)identity
                  expectedLabel:(NSString *)label
                           mode:(ObservationMode)mode
                maximumEncoders:(NSUInteger)maximum
                 counterFactory:(ObservationCounterFactory)factory;
- (void)sealCommandBuffer:(id<MTLCommandBuffer>)buffer;
- (NSDictionary *)completeCommandBuffer:(id<MTLCommandBuffer>)buffer;
- (NSDictionary *)snapshot;
@end

@interface ObservedCommandBuffer : NSProxy
+ (id<MTLCommandBuffer>)wrap:(id<MTLCommandBuffer>)buffer ledger:(ObservationLedger *)ledger;
@end

// The owner, never the proxy, performs submission. False means already submitted.
BOOL ObservationCommitIfNeeded(id<MTLCommandBuffer> buffer);

NS_ASSUME_NONNULL_END
