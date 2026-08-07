const std = @import("std");
pub const pty = @import("pty.zig");
pub const pane = @import("pane.zig");
pub const layout = @import("layout.zig");
pub const terminal = @import("terminal.zig");
pub const renderer = @import("renderer.zig");
pub const app = @import("app.zig");
pub const mac_app = @import("gui/mac_app.zig");

pub fn main() !void {
    std.debug.print("Launching Neomux Native macOS GUI Application...\n", .{});

    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();

    var desktop_app = try mac_app.MacApp.init();
    std.debug.print("Native window created successfully! Running NSApplication event loop...\n", .{});

    desktop_app.run();
}

test {
    std.testing.refAllDecls(@This());
    _ = pty;
    _ = pane;
    _ = layout;
    _ = terminal;
    _ = renderer;
    _ = app;
    _ = mac_app;
}
