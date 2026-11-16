// SPDX-License-Identifier: MIT
#import <AppKit/AppKit.h>
#import <Security/Authorization.h>
#import <Security/AuthorizationTags.h>
#include "ShaderCacheState.h"

static NSString *text(NSString *english, NSString *chinese) {
    return [NSLocale.preferredLanguages.firstObject hasPrefix:@"zh"] ? chinese : english;
}

@interface ShaderCacheApp : NSObject <NSApplicationDelegate>
@property NSWindow *window;
@property NSTextField *state;
@property NSTextField *counts;
@property NSTextField *current;
@property NSProgressIndicator *progress;
@property NSButton *start;
@property NSButton *stop;
@property NSTimer *timer;
@property BOOL authorizing;
@property AuthorizationRef authorization;
@end

@implementation ShaderCacheApp
- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    (void)notification;
    NSMenu *menu = [NSMenu new];
    NSMenuItem *application = [NSMenuItem new];
    [menu addItem:application];
    application.submenu = [NSMenu new];
    [application.submenu addItemWithTitle:text(@"Quit Shader Cache", @"退出着色器缓存") action:@selector(terminate:) keyEquivalent:@"q"];
    NSApp.mainMenu = menu;
    self.window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,420,228)
        styleMask:NSWindowStyleMaskTitled|NSWindowStyleMaskClosable|NSWindowStyleMaskMiniaturizable backing:NSBackingStoreBuffered defer:NO];
    self.window.title = text(@"Shader Cache", @"着色器缓存");
    self.window.releasedWhenClosed = NO;
    self.state = [NSTextField labelWithString:@""];
    self.state.font = [NSFont systemFontOfSize:15 weight:NSFontWeightMedium];
    self.progress = [NSProgressIndicator new];
    self.progress.indeterminate = NO;
    self.progress.minValue = 0;
    self.progress.maxValue = 1;
    self.counts = [NSTextField labelWithString:@""];
    self.counts.font = [NSFont monospacedDigitSystemFontOfSize:13 weight:NSFontWeightRegular];
    self.counts.maximumNumberOfLines = 2;
    self.current = [NSTextField labelWithString:@""];
    self.current.lineBreakMode = NSLineBreakByTruncatingMiddle;
    self.current.maximumNumberOfLines = 1;
    self.current.font = [NSFont systemFontOfSize:11];
    self.current.textColor = NSColor.secondaryLabelColor;
    self.start = [NSButton buttonWithTitle:text(@"Start", @"开始") target:self action:@selector(startCompilation:)];
    self.start.keyEquivalent = @"\r";
    self.stop = [NSButton buttonWithTitle:text(@"Stop", @"停止") target:self action:@selector(stopCompilation:)];
    NSStackView *buttons = [NSStackView stackViewWithViews:@[self.start,self.stop]];
    buttons.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    buttons.spacing = 8;
    NSStackView *stack = [NSStackView stackViewWithViews:@[self.state,self.progress,self.counts,self.current,buttons]];
    stack.orientation = NSUserInterfaceLayoutOrientationVertical;
    stack.alignment = NSLayoutAttributeLeading;
    stack.spacing = 16;
    stack.translatesAutoresizingMaskIntoConstraints = NO;
    [self.window.contentView addSubview:stack];
    [NSLayoutConstraint activateConstraints:@[
        [stack.leadingAnchor constraintEqualToAnchor:self.window.contentView.leadingAnchor constant:24],
        [stack.trailingAnchor constraintEqualToAnchor:self.window.contentView.trailingAnchor constant:-24],
        [stack.topAnchor constraintEqualToAnchor:self.window.contentView.topAnchor constant:24],
        [self.progress.widthAnchor constraintEqualToAnchor:stack.widthAnchor],
        [self.current.widthAnchor constraintEqualToAnchor:stack.widthAnchor]
    ]];
    self.timer = [NSTimer scheduledTimerWithTimeInterval:1 target:self selector:@selector(refresh) userInfo:nil repeats:YES];
    [self refresh];
    [self.window center];
    [self.window makeKeyAndOrderFront:nil];
    [NSApp activateIgnoringOtherApps:YES];
}
- (void)refresh {
    NSDictionary *value = SCReadState();
    NSDictionary *labels = @{
        @"idle":text(@"Not started", @"未开始"), @"running":text(@"Compiling", @"正在编译"),
        @"stopping":text(@"Stopping", @"正在停止"), @"stopped":text(@"Stopped", @"已停止"),
        @"interrupted":text(@"Interrupted", @"已中断"), @"complete":text(@"Complete", @"已完成"),
        @"incomplete":text(@"Incomplete", @"未完成"), @"failed":text(@"Failed", @"失败"),
        @"outdated":text(@"Update available", @"缓存待更新")
    };
    self.state.stringValue = self.authorizing ? text(@"Authorizing", @"正在授权") : labels[value[@"state"]] ?: text(@"Not started", @"未开始");
    double total = [value[@"total"] doubleValue];
    [NSAnimationContext runAnimationGroup:^(NSAnimationContext *context) {
        context.duration = 0.2;
        [[self.progress animator] setDoubleValue:total ? [value[@"libraries"] doubleValue] / total : 0];
    } completionHandler:nil];
    self.counts.stringValue = [NSString stringWithFormat:text(@"Libraries %@/%@  Functions %@\nFailed libraries %@  Functions %@  Missing %@", @"库 %@/%@  函数 %@\n失败库 %@  失败函数 %@  缺失 %@"),
        value[@"libraries"] ?: @0,value[@"total"] ?: @0,value[@"functions"] ?: @0,value[@"failedLibraries"] ?: @0,value[@"failedFunctions"] ?: @0,value[@"missing"] ?: @0];
    self.current.stringValue = [value[@"error"] length] ? value[@"error"] : [value[@"function"] length] ? value[@"function"] : value[@"library"] ?: @"";
    self.current.toolTip = self.current.stringValue;
    self.start.enabled = ![value[@"running"] boolValue] && !self.authorizing;
    self.stop.enabled = [value[@"running"] boolValue] && !self.authorizing && ![value[@"state"] isEqual:@"stopping"];
}
- (void)perform:(const char *)operation {
    if (self.authorizing) return;
    self.authorizing = YES;
    [self refresh];
    NSString *worker = [NSBundle.mainBundle.bundlePath stringByAppendingPathComponent:@"Contents/MacOS/ShaderCacheWorker"];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        OSStatus status = errAuthorizationSuccess;
        AuthorizationRef authorization = self.authorization;
        if (!authorization) status = AuthorizationCreate(nullptr, kAuthorizationEmptyEnvironment, kAuthorizationFlagDefaults, &authorization);
        if (status == errAuthorizationSuccess) {
            AuthorizationItem item = {kAuthorizationRightExecute,0,nullptr,0};
            AuthorizationRights rights = {1,&item};
            status = AuthorizationCopyRights(authorization,&rights,kAuthorizationEmptyEnvironment,
                kAuthorizationFlagInteractionAllowed|kAuthorizationFlagExtendRights|kAuthorizationFlagPreAuthorize,nullptr);
        }
        if (status == errAuthorizationSuccess) {
            char *arguments[] = {const_cast<char *>(operation),nullptr};
            FILE *channel = nullptr;
            status = AuthorizationExecuteWithPrivileges(authorization,worker.fileSystemRepresentation,kAuthorizationFlagDefaults,arguments,&channel);
            if (channel) fclose(channel);
        }
        dispatch_async(dispatch_get_main_queue(), ^{
            self.authorization = authorization;
            self.authorizing = NO;
            [self refresh];
            if (status != errAuthorizationSuccess && status != errAuthorizationCanceled) {
                NSAlert *alert = [NSAlert new];
                alert.messageText = text(@"Could not start the worker", @"无法启动工作进程");
                alert.informativeText = [NSString stringWithFormat:@"%d",(int)status];
                [alert beginSheetModalForWindow:self.window completionHandler:nil];
            }
        });
    });
}
- (void)startCompilation:(id)sender {(void)sender; [self perform:"--run"];}
- (void)stopCompilation:(id)sender {(void)sender; [self perform:"--stop"];}
- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {(void)sender; return YES;}
@end

int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc == 2 && !strcmp(argv[1], "--status")) {
            NSMutableDictionary *value = SCReadState().mutableCopy;
            [value removeObjectForKey:@"updated"];
            NSData *json = [NSJSONSerialization dataWithJSONObject:value options:NSJSONWritingPrettyPrinted error:nil];
            fwrite(json.bytes,1,json.length,stdout);
            putchar('\n');
            return 0;
        }
        NSApplication *application = NSApplication.sharedApplication;
        ShaderCacheApp *delegate = [ShaderCacheApp new];
        application.delegate = delegate;
        [application setActivationPolicy:NSApplicationActivationPolicyRegular];
        [application run];
    }
    return 0;
}
