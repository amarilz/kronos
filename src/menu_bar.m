#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>

void kronos_update_menu_bar(const char *clock) {
    static NSStatusItem *item;

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
            item.button.toolTip = @"Kronos study timer";
            item.button.font = [NSFont monospacedDigitSystemFontOfSize:NSFont.systemFontSize
                                                               weight:NSFontWeightRegular];
        }

        NSString *title = [NSString stringWithFormat:@"⏱ %s", clock];
        if (![item.button.title isEqualToString:title]) {
            item.button.title = title;
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
