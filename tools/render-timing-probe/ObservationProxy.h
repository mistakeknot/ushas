#import <Foundation/Foundation.h>
#import <Metal/Metal.h>

NS_ASSUME_NONNULL_BEGIN

typedef NS_ENUM(NSUInteger, ObservationMode) {
    ObservationModeCalls,
    ObservationModeCounters,
};
typedef id<MTLCounterSampleBuffer> _Nullable (^ObservationCounterFactory)(NSString *label, NSUInteger count, NSError * _Nullable * _Nullable error);

// Internal to the isolated probe. No Ushas or general-purpose API is introduced.
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
