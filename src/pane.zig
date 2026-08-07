const std = @import("std");
const pty = @import("pty.zig");

pub const PaneId = u32;

pub const Pane = struct {
    id: PaneId,
    title: []const u8,
    pty_process: pty.Pty,
    is_focused: bool,
    allocator: std.mem.Allocator,

    pub fn create(allocator: std.mem.Allocator, id: PaneId, title: []const u8, winsize: pty.Winsize) !*Pane {
        const pane_ptr = try allocator.create(Pane);
        errdefer allocator.destroy(pane_ptr);

        const duped_title = try allocator.dupe(u8, title);
        errdefer allocator.free(duped_title);

        const pty_proc = try pty.Pty.spawn(allocator, null, winsize);

        pane_ptr.* = Pane{
            .id = id,
            .title = duped_title,
            .pty_process = pty_proc,
            .is_focused = false,
            .allocator = allocator,
        };

        return pane_ptr;
    }

    pub fn destroy(self: *Pane) void {
        self.pty_process.deinit();
        self.allocator.free(self.title);
        self.allocator.destroy(self);
    }
};

test "pane creation and destruction" {
    const testing = std.testing;
    const pane_inst = try Pane.create(testing.allocator, 1, "Terminal 1", .{ .ws_row = 24, .ws_col = 80 });
    defer pane_inst.destroy();

    try testing.expectEqual(@as(PaneId, 1), pane_inst.id);
    try testing.expectEqualStrings("Terminal 1", pane_inst.title);
}
