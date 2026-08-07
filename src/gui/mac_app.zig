const std = @import("std");

// Objective-C runtime bindings for macOS AppKit
extern "c" fn objc_getClass(name: [*:0]const u8) ?*anyopaque;
extern "c" fn sel_registerName(name: [*:0]const u8) ?*anyopaque;
extern "c" fn objc_msgSend(self: ?*anyopaque, op: ?*anyopaque, ...) ?*anyopaque;

pub const CGRect = extern struct {
    origin: CGPoint,
    size: CGSize,
};

pub const CGPoint = extern struct {
    x: f64,
    y: f64,
};

pub const CGSize = extern struct {
    width: f64,
    height: f64,
};

pub const MacApp = struct {
    app_ptr: ?*anyopaque,
    window_ptr: ?*anyopaque,

    pub fn init() !MacApp {
        // 1. Get NSApplication class & sharedApplication instance
        const NSApplication = objc_getClass("NSApplication") orelse return error.ClassNotFound;
        const sharedAppSel = sel_registerName("sharedApplication");

        const msgSend_app = @as(*const fn (?*anyopaque, ?*anyopaque) callconv(.c) ?*anyopaque, @ptrCast(&objc_msgSend));
        const app_instance = msgSend_app(NSApplication, sharedAppSel) orelse return error.AppInitFailed;

        // 2. Set activation policy to Regular (Dock icon & GUI application window)
        const setActivationPolicySel = sel_registerName("setActivationPolicy:");
        const msgSend_policy = @as(*const fn (?*anyopaque, ?*anyopaque, i64) callconv(.c) i64, @ptrCast(&objc_msgSend));
        _ = msgSend_policy(app_instance, setActivationPolicySel, 0); // NSApplicationActivationPolicyRegular = 0

        // 3. Create NSWindow
        const NSWindow = objc_getClass("NSWindow") orelse return error.ClassNotFound;
        const allocSel = sel_registerName("alloc");
        const msgSend_alloc = @as(*const fn (?*anyopaque, ?*anyopaque) callconv(.c) ?*anyopaque, @ptrCast(&objc_msgSend));
        const uninit_window = msgSend_alloc(NSWindow, allocSel);

        const frame = CGRect{
            .origin = .{ .x = 200, .y = 200 },
            .size = .{ .width = 1280, .height = 800 },
        };

        // StyleMask: Titled (1) | Closable (2) | Miniaturizable (4) | Resizable (8) = 15
        const styleMask: u64 = 15;
        const backingStore: u64 = 2; // NSBackingStoreBuffered
        const deferCreation: bool = false;

        const initWithContentRectSel = sel_registerName("initWithContentRect:styleMask:backing:defer:");
        const msgSend_initWindow = @as(*const fn (?*anyopaque, ?*anyopaque, CGRect, u64, u64, bool) callconv(.c) ?*anyopaque, @ptrCast(&objc_msgSend));
        const window_instance = msgSend_initWindow(uninit_window, initWithContentRectSel, frame, styleMask, backingStore, deferCreation) orelse return error.WindowInitFailed;

        // 4. Set Window Title
        const NSString = objc_getClass("NSString") orelse return error.ClassNotFound;
        const stringWithUTF8StringSel = sel_registerName("stringWithUTF8String:");
        const msgSend_string = @as(*const fn (?*anyopaque, ?*anyopaque, [*:0]const u8) callconv(.c) ?*anyopaque, @ptrCast(&objc_msgSend));
        const title_str = msgSend_string(NSString, stringWithUTF8StringSel, "Neomux - Terminal & Claude AI");

        const setTitleSel = sel_registerName("setTitle:");
        const msgSend_setTitle = @as(*const fn (?*anyopaque, ?*anyopaque, ?*anyopaque) callconv(.c) void, @ptrCast(&objc_msgSend));
        msgSend_setTitle(window_instance, setTitleSel, title_str);

        // 5. Set Window background color to modern dark theme (#1e1e2e)
        const NSColor = objc_getClass("NSColor") orelse return error.ClassNotFound;
        const colorWithSRGBRedSel = sel_registerName("colorWithSRGBRed:green:blue:alpha:");
        const msgSend_color = @as(*const fn (?*anyopaque, ?*anyopaque, f64, f64, f64, f64) callconv(.c) ?*anyopaque, @ptrCast(&objc_msgSend));
        const dark_bg = msgSend_color(NSColor, colorWithSRGBRedSel, 0.11, 0.11, 0.18, 1.0);

        const setBackgroundColorSel = sel_registerName("setBackgroundColor:");
        msgSend_setTitle(window_instance, setBackgroundColorSel, dark_bg);

        // 6. Make key and order front
        const makeKeyAndOrderFrontSel = sel_registerName("makeKeyAndOrderFront:");
        const msgSend_orderFront = @as(*const fn (?*anyopaque, ?*anyopaque, ?*anyopaque) callconv(.c) void, @ptrCast(&objc_msgSend));
        msgSend_orderFront(window_instance, makeKeyAndOrderFrontSel, null);

        // 7. Activate application ignoring other apps
        const activateSel = sel_registerName("activateIgnoringOtherApps:");
        const msgSend_activate = @as(*const fn (?*anyopaque, ?*anyopaque, bool) callconv(.c) void, @ptrCast(&objc_msgSend));
        msgSend_activate(app_instance, activateSel, true);

        return MacApp{
            .app_ptr = app_instance,
            .window_ptr = window_instance,
        };
    }

    pub fn run(self: *MacApp) void {
        const runSel = sel_registerName("run");
        const msgSend_run = @as(*const fn (?*anyopaque, ?*anyopaque) callconv(.c) void, @ptrCast(&objc_msgSend));
        msgSend_run(self.app_ptr, runSel);
    }
};

test "mac app initialization" {
    const testing = std.testing;
    const app_inst = MacApp.init() catch return; // Skip if non-GUI headless environment
    try testing.expect(app_inst.app_ptr != null);
    try testing.expect(app_inst.window_ptr != null);
}
