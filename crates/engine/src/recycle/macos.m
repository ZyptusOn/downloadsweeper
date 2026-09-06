#import <Foundation/Foundation.h>
#include <string.h>

// Narrow Foundation adapter; Rust owns selection, validation, journalling and undo.
int ds_recycle_macos(const char *path, int moving, char *output, size_t capacity) {
    @autoreleasepool {
        NSString *name = [[NSFileManager defaultManager] stringWithFileSystemRepresentation:path length:strlen(path)];
        if (!name) return 1;
        NSURL *source = [NSURL fileURLWithPath:name];
        NSURL *result = nil;
        NSError *error = nil;
        NSFileManager *manager = [NSFileManager defaultManager];
        if (moving) {
            if (![manager trashItemAtURL:source resultingItemURL:&result error:&error]) return 2;
        } else {
            result = [manager URLForDirectory:NSTrashDirectory inDomain:NSUserDomainMask appropriateForURL:source create:YES error:&error];
        }
        if (!result) return 3;
        const char *value = result.fileSystemRepresentation;
        if (!value || strlen(value) + 1 > capacity) return 4;
        memcpy(output, value, strlen(value) + 1);
        return 0;
    }
}
