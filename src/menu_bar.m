#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#include <stdbool.h>

void kronos_update_menu_bar(const char *clock, bool filled) {
    static NSStatusItem *item;
    static NSImage *outlineIcon;
    static NSImage *filledIcon;

    @autoreleasepool {
        if (clock == NULL) {
            if (item != nil) {
                [NSStatusBar.systemStatusBar removeStatusItem:item];
                item = nil;
            }
            return;
        }

        if (item == nil) {
            // A terminal can also run without access to a graphical login session.
            CFDictionaryRef session = CGSessionCopyCurrentDictionary();
            if (session == NULL) {
                return;
            }
            CFRelease(session);

            [NSApplication.sharedApplication setActivationPolicy:NSApplicationActivationPolicyAccessory];
            item = [NSStatusBar.systemStatusBar statusItemWithLength:NSVariableStatusItemLength];
            if (@available(macOS 11.0, *)) {
                outlineIcon = [NSImage imageWithSystemSymbolName:@"stopwatch"
                                       accessibilityDescription:nil];
                filledIcon = [NSImage imageWithSystemSymbolName:@"stopwatch.fill"
                                      accessibilityDescription:nil];
                item.button.imagePosition = NSImageLeft;
            }
            item.button.toolTip = @"Kronos study timer";
            item.button.font = [NSFont monospacedDigitSystemFontOfSize:NSFont.systemFontSize
                                                               weight:NSFontWeightRegular];
        }

        bool hasStopwatchIcons = outlineIcon != nil && filledIcon != nil;
        NSString *title = hasStopwatchIcons ? [NSString stringWithUTF8String:clock]
            : [NSString stringWithFormat:@"%@ %s", filled ? @"●" : @"○", clock];
        if (![item.button.title isEqualToString:title]) {
            item.button.title = title;
            if (hasStopwatchIcons) {
                item.button.image = filled ? filledIcon : outlineIcon;
            }
        }

        // Service AppKit on the main thread without waiting for GUI input.
        NSEvent *event;
        while ((event = [NSApp nextEventMatchingMask:NSEventMaskAny
                                           untilDate:NSDate.distantPast
                                              inMode:NSDefaultRunLoopMode
                                             dequeue:YES]) != nil) {
            [NSApp sendEvent:event];
        }
        [NSApp updateWindows];
    }
}
