// System PDF rasterization from a verified descriptor; no viewer, actions or external URLs.
#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <ImageIO/ImageIO.h>
#include <sys/stat.h>
#include <unistd.h>
#include <stdio.h>
#include <math.h>
#include <stdint.h>
#include <string.h>

typedef struct { int fd; off_t size; size_t read; CFAbsoluteTime started; } DSPDFSource;
static size_t DSReadPDF(void *info, void *buffer, off_t position, size_t count) {
    DSPDFSource *s=info;
    if (position<0 || position>=s->size || CFAbsoluteTimeGetCurrent()-s->started>6) return 0;
    count=MIN(count,(size_t)(s->size-position));
    if (count>64*1024*1024-s->read) return 0;
    ssize_t n=pread(s->fd,buffer,count,position);
    if(n<=0) return 0;
    s->read+=(size_t)n;return (size_t)n;
}
static NSData *DSPagePacket(CGContextRef context,NSArray *sampled,size_t count,NSUInteger requested) {
    CGImageRef image=CGBitmapContextCreateImage(context);
    if(!image) return nil;
    NSData *encoded=nil;
    for(NSNumber *quality in @[@0.75,@0.55,@0.35]) {
        NSMutableData *data=[NSMutableData new];
        CGImageDestinationRef destination=CGImageDestinationCreateWithData((__bridge CFMutableDataRef)data,CFSTR("public.jpeg"),1,NULL);
        if(!destination) break;
        CGImageDestinationAddImage(destination,image,(__bridge CFDictionaryRef)@{(__bridge NSString *)kCGImageDestinationLossyCompressionQuality:quality});
        BOOL ok=CGImageDestinationFinalize(destination);CFRelease(destination);
        if(ok && data.length<=384*1024) {encoded=data;break;}
    }
    CGImageRelease(image);
    if(!encoded) return nil;
    return [NSJSONSerialization dataWithJSONObject:@{
        @"image":@{@"mime":@"image/jpeg",@"data_base64":[encoded base64EncodedStringWithOptions:0],@"high_detail":@YES},
        @"info":@{@"status":@"sampled",@"kind":@"pdf_page_samples",@"backend":@"coregraphics_pdf",
            @"page_count":@(count),@"sampled_pages":[sampled copy],@"layout":@"left_to_right",@"page_max_px":@[@512,@768],
            @"partial":@(sampled.count<requested),@"full_document":@NO,
            @"message":@"从左到右对应 sampled_pages 中的页码；只预览首页、第二页和中间页，不代表全文。不执行脚本、附件或 OCR。"}}
        options:0 error:NULL];
}
static NSData *DSRenderPDF(int descriptor) {
    struct stat st;
    if(fstat(descriptor,&st)!=0 || !S_ISREG(st.st_mode) || st.st_size<12 || st.st_size>64*1024*1024) return nil;
    DSPDFSource source={descriptor,st.st_size,0,CFAbsoluteTimeGetCurrent()};
    CGDataProviderDirectCallbacks callbacks={0,NULL,NULL,DSReadPDF,NULL};
    CGDataProviderRef provider=CGDataProviderCreateDirect(&source,st.st_size,&callbacks);
    if(!provider) return nil;
    CGPDFDocumentRef document=CGPDFDocumentCreateWithProvider(provider);
    CGDataProviderRelease(provider);
    if(!document) return nil;
    size_t count=CGPDFDocumentGetNumberOfPages(document);
    if(!count || count>100000 || CGPDFDocumentIsEncrypted(document)) {CGPDFDocumentRelease(document);return nil;}
    NSMutableArray *targets=[NSMutableArray new];
    for(NSNumber *p in @[@1,@2,@(count/2+1)]) if(p.unsignedLongLongValue<=count && ![targets containsObject:p]) [targets addObject:p];
    CGColorSpaceRef color=CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    if(!color) {CGPDFDocumentRelease(document);return nil;}
    CGContextRef ctx=CGBitmapContextCreate(NULL,512*targets.count,768,8,512*targets.count*4,color,kCGImageAlphaNoneSkipLast);
    CGColorSpaceRelease(color);
    if(!ctx) {CGPDFDocumentRelease(document);return nil;}
    CGContextSetRGBFillColor(ctx,1,1,1,1);CGContextFillRect(ctx,CGRectMake(0,0,512*targets.count,768));
    NSMutableArray *sampled=[NSMutableArray new];NSData *latest=nil;
    for(NSNumber *target in targets) {
        if(CFAbsoluteTimeGetCurrent()-source.started>6) break;
        CGPDFPageRef page=CGPDFDocumentGetPage(document,target.unsignedLongLongValue);
        if(!page) continue;
        CGRect box=CGPDFPageGetBoxRect(page,kCGPDFCropBox);
        if(!isfinite(box.size.width) || !isfinite(box.size.height) || box.size.width<=0 || box.size.height<=0) continue;
        CGContextSaveGState(ctx);
        CGRect slot=CGRectMake(512*sampled.count,0,512,768);
        CGContextClipToRect(ctx,slot);
        CGContextConcatCTM(ctx,CGPDFPageGetDrawingTransform(page,kCGPDFCropBox,slot,0,true));
        CGContextDrawPDFPage(ctx,page);CGContextRestoreGState(ctx);
        [sampled addObject:target];latest=DSPagePacket(ctx,sampled,count,targets.count);
        if(latest) {fwrite(latest.bytes,1,latest.length,stdout);fputc('\n',stdout);fflush(stdout);}
    }
    CGContextRelease(ctx);CGPDFDocumentRelease(document);return latest;
}
int ds_macos_pdf(int descriptor,uint8_t *output,size_t capacity,size_t *length) {
    @autoreleasepool { @try {
        NSData *packet=DSRenderPDF(descriptor);
        if(!packet || packet.length>capacity) return 2;
        memcpy(output,packet.bytes,packet.length);*length=packet.length;return 0;
    } @catch(NSException *exception) {(void)exception;return 2;} }
}
