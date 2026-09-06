// Compiled into the application on macOS only; no Swift executable or extra runtime.
// AVFoundation receives a custom resource backed exclusively by a verified descriptor.
#import <Foundation/Foundation.h>
#import <AVFoundation/AVFoundation.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreGraphics/CoreGraphics.h>
#import <ImageIO/ImageIO.h>
#include <sys/stat.h>
#include <unistd.h>
#include <math.h>
#include <string.h>
#include <stdint.h>

@interface DSPreviewResource : NSObject <AVAssetResourceLoaderDelegate>
@property(nonatomic) int descriptor;
@property(nonatomic) int64_t length;
@property(nonatomic) NSUInteger bytesRead;
@property(nonatomic, copy) NSString *contentType;
@property(nonatomic, strong) NSURL *resourceURL;
@end

@implementation DSPreviewResource
- (void)dealloc { if (_descriptor >= 0) close(_descriptor); }
- (BOOL)resourceLoader:(AVAssetResourceLoader *)loader
    shouldWaitForLoadingOfRequestedResource:(AVAssetResourceLoadingRequest *)request {
    (void)loader;
    @autoreleasepool {
        // Never resolve secondary resources, redirects, HTTP, or file URLs.
        if (![request.request.URL isEqual:self.resourceURL]) {
            [request finishLoadingWithError:[NSError errorWithDomain:@"DSPreview" code:1 userInfo:nil]];
            return YES;
        }
        AVAssetResourceLoadingContentInformationRequest *info = request.contentInformationRequest;
        if (info) {
            info.contentType = self.contentType;
            info.contentLength = self.length;
            info.byteRangeAccessSupported = YES;
        }
        AVAssetResourceLoadingDataRequest *data = request.dataRequest;
        if (data) {
            int64_t offset = MAX(data.requestedOffset, data.currentOffset);
            int64_t end = self.length;
            if (!data.requestsAllDataToEndOfResource) {
                if (data.requestedOffset < 0 || data.requestedLength < 0 ||
                    data.requestedOffset > INT64_MAX - data.requestedLength) goto fail;
                end = MIN(self.length, data.requestedOffset + data.requestedLength);
            }
            if (offset < 0 || offset > self.length || end < offset) goto fail;
            // A decoder asking for a whole large file exceeds the lightweight budget.
            if ((uint64_t)(end - offset) > 64 * 1024 * 1024 - self.bytesRead) goto fail;
            uint8_t chunk[64 * 1024];
            while (offset < end) {
                if (request.cancelled) return YES;
                size_t count = (size_t)MIN((int64_t)sizeof(chunk), end - offset);
                ssize_t received = pread(self.descriptor, chunk, count, (off_t)offset);
                if (received <= 0) goto fail;
                self.bytesRead += (NSUInteger)received;
                [data respondWithData:[NSData dataWithBytes:chunk length:(NSUInteger)received]];
                offset += received;
            }
        }
        [request finishLoading];
        return YES;
    fail:
        [request finishLoadingWithError:[NSError errorWithDomain:@"DSPreview" code:2 userInfo:nil]];
        return YES;
    }
}
@end

static NSData *DSGenerate(int descriptor) {
    struct stat snapshot;
    if (fstat(descriptor, &snapshot) != 0 || !S_ISREG(snapshot.st_mode) || snapshot.st_size < 12) return nil;
    uint8_t header[12];
    if (pread(descriptor, header, sizeof(header), 0) != sizeof(header)) return nil;
    NSString *type;
    if (memcmp(header + 4, "ftyp", 4) == 0) {
        type = memcmp(header + 8, "qt  ", 4) == 0 ? @"com.apple.quicktime-movie" : @"public.mpeg-4";
    } else if (memcmp(header, "RIFF", 4) == 0 && memcmp(header + 8, "AVI ", 4) == 0) {
        type = @"public.avi";
    } else return nil; // Unsupported containers use the optional FFmpeg fallback.
    int owned = dup(descriptor);
    if (owned < 0) return nil;
    // AVAssetResourceLoader keeps its delegate weakly. Prevent ARC from releasing
    // the descriptor owner after setDelegate while asynchronous decoding is running.
    __attribute__((objc_precise_lifetime)) DSPreviewResource *resource = [DSPreviewResource new];
    resource.descriptor = owned;
    resource.length = snapshot.st_size;
    resource.contentType = type;
    NSString *extension = [type isEqualToString:@"public.avi"] ? @"avi" :
        ([type isEqualToString:@"com.apple.quicktime-movie"] ? @"mov" : @"mp4");
    resource.resourceURL = [NSURL URLWithString:[@"ds-preview://asset/video." stringByAppendingString:extension]];
    AVURLAsset *asset = [AVURLAsset URLAssetWithURL:resource.resourceURL
        options:@{AVURLAssetReferenceRestrictionsKey: @(AVAssetReferenceRestrictionForbidAll),
                  AVURLAssetPreferPreciseDurationAndTimingKey: @NO}];
    dispatch_queue_t queue = dispatch_queue_create("com.downloadsweeper.preview.read", DISPATCH_QUEUE_SERIAL);
    [asset.resourceLoader setDelegate:resource queue:queue];
    // Use APIs available on macOS 11, including the first Apple Silicon machines.
    dispatch_semaphore_t loaded = dispatch_semaphore_create(0);
    [asset loadValuesAsynchronouslyForKeys:@[@"duration", @"tracks"] completionHandler:^{ dispatch_semaphore_signal(loaded); }];
    if (dispatch_semaphore_wait(loaded, dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC)) != 0) {
        [asset cancelLoading]; return nil;
    }
    if ([asset statusOfValueForKey:@"duration" error:NULL] != AVKeyValueStatusLoaded ||
        [asset statusOfValueForKey:@"tracks" error:NULL] != AVKeyValueStatusLoaded) return nil;
    double duration = CMTimeGetSeconds(asset.duration);
    if (!isfinite(duration) || duration <= 0 || duration >= 31536000) return nil;
    NSArray<AVAssetTrack *> *tracks = [asset tracksWithMediaType:AVMediaTypeVideo];
    if (tracks.count == 0) return nil;
    CGSize sourceSize = tracks.firstObject.naturalSize;
    if (!isfinite(sourceSize.width) || !isfinite(sourceSize.height) || sourceSize.width <= 0 ||
        sourceSize.height <= 0 || sourceSize.width > 8192 || sourceSize.height > 8192) return nil;
    AVAssetImageGenerator *generator = [AVAssetImageGenerator assetImageGeneratorWithAsset:asset];
    generator.maximumSize = CGSizeMake(256, 256);
    generator.appliesPreferredTrackTransform = YES;
    // Classification needs representative keyframes, not frame-accurate seeking through a long GOP.
    generator.requestedTimeToleranceBefore = kCMTimePositiveInfinity;
    generator.requestedTimeToleranceAfter = kCMTimePositiveInfinity;
    double candidates[3] = {0, MIN(duration * 0.1, 1.0), duration * 0.5};
    NSMutableArray<NSValue *> *times = [NSMutableArray new];
    for (int i = 0; i < 3; i++) {
        if (times.count && candidates[i] - CMTimeGetSeconds(times.lastObject.CMTimeValue) < 0.25) continue;
        [times addObject:[NSValue valueWithCMTime:CMTimeMakeWithSeconds(candidates[i], 600)]];
    }
    NSMutableArray *results = [NSMutableArray new];
    dispatch_group_t group = dispatch_group_create();
    for (NSUInteger i = 0; i < times.count; i++) dispatch_group_enter(group);
    [generator generateCGImagesAsynchronouslyForTimes:times completionHandler:
        ^(CMTime requested, CGImageRef image, CMTime actual, AVAssetImageGeneratorResult status, NSError *error) {
        (void)error;
        @autoreleasepool {
            if (status == AVAssetImageGeneratorSucceeded && image && CGImageGetWidth(image) <= 256 && CGImageGetHeight(image) <= 256) {
                // NSArray retains the bridged image before the callback's image expires.
                @synchronized(results) {
                    [results addObject:@{ @"target": @(CMTimeGetSeconds(requested)),
                        @"actual": @(CMTimeGetSeconds(actual)), @"image": (__bridge id)image }];
                }
            }
            dispatch_group_leave(group);
        }
    }];
    BOOL partial = dispatch_group_wait(group, dispatch_time(DISPATCH_TIME_NOW, 4 * NSEC_PER_SEC)) != 0;
    if (partial) { [generator cancelAllCGImageGeneration]; [asset cancelLoading]; }
    // Callbacks may still drain after cancellation. Work only on a synchronized snapshot.
    @synchronized(results) { results = [results mutableCopy]; }
    if (results.count == 0) return nil;
    [results sortUsingComparator:^NSComparisonResult(NSDictionary *a, NSDictionary *b) { return [a[@"target"] compare:b[@"target"]]; }];
    CGColorSpaceRef color = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    if (!color) return nil;
    CGContextRef context = CGBitmapContextCreate(NULL, 512, 512, 8, 512 * 4, color, kCGImageAlphaNoneSkipLast);
    CGColorSpaceRelease(color);
    if (!context) return nil;
    CGContextSetRGBFillColor(context, 245.0/255, 245.0/255, 245.0/255, 1);
    CGContextFillRect(context, CGRectMake(0, 0, 512, 512));
    NSMutableArray *sampled = [NSMutableArray new], *actualTimes = [NSMutableArray new];
    for (NSUInteger i = 0; i < MIN(results.count, (NSUInteger)3); i++) {
        NSDictionary *frame = results[i];
        CGImageRef image = (__bridge CGImageRef)frame[@"image"];
        CGFloat w = CGImageGetWidth(image), h = CGImageGetHeight(image);
        CGContextDrawImage(context, CGRectMake((i % 2) * 256 + (256-w)/2, 512 - ((i / 2) * 256 + (256-h)/2 + h), w, h), image);
        [sampled addObject:frame[@"target"]];
        [actualTimes addObject:frame[@"actual"]];
    }
    CGImageRef sheet = CGBitmapContextCreateImage(context);
    CGContextRelease(context);
    if (!sheet) return nil;
    NSMutableData *jpeg = [NSMutableData new];
    CGImageDestinationRef destination = CGImageDestinationCreateWithData((__bridge CFMutableDataRef)jpeg, CFSTR("public.jpeg"), 1, NULL);
    if (!destination) { CGImageRelease(sheet); return nil; }
    CGImageDestinationAddImage(destination, sheet, (__bridge CFDictionaryRef)@{(__bridge NSString *)kCGImageDestinationLossyCompressionQuality: @0.65});
    BOOL ok = CGImageDestinationFinalize(destination);
    CFRelease(destination); CGImageRelease(sheet);
    if (!ok || jpeg.length > 192 * 1024) return nil;
    NSDictionary *visual = @{@"image": @{@"mime": @"image/jpeg", @"data_base64": [jpeg base64EncodedStringWithOptions:0]},
        @"info": @{@"status": @"sampled", @"kind": @"video_contact_sheet", @"backend": @"avfoundation",
            @"sample_targets_seconds": sampled, @"actual_times_seconds": actualTimes,
            @"duration_seconds": @(duration), @"layout": @"row_major_2x2", @"partial": @(partial || sampled.count < times.count),
            @"message": @"系统原生采样；左上、右上、左下为开头及中段附近帧。定位允许误差，空白格不是帧；无音频，不代表完整视频。"}};
    return [NSJSONSerialization dataWithJSONObject:visual options:0 error:NULL];
}

int ds_macos_preview(int descriptor, uint8_t *output, size_t capacity, size_t *length) {
    @autoreleasepool {
        @try {
            NSData *json = DSGenerate(descriptor);
            if (!json || json.length > capacity) return 2;
            memcpy(output, json.bytes, json.length);
            *length = json.length;
            return 0;
        } @catch (NSException *exception) { (void)exception; return 2; }
    }
}
