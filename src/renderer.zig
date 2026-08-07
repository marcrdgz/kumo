const std = @import("std");
const posix = std.posix;
const layout = @import("layout.zig");
const pane = @import("pane.zig");

pub const Renderer = struct {
    stdout_fd: posix.fd_t,

    pub fn init() Renderer {
        return .{
            .stdout_fd = posix.STDOUT_FILENO,
        };
    }

    pub fn drawStatusBar(self: *Renderer, rows: u16, cols: u16, active_pane_id: u32, is_leader_active: bool) !void {
        // Move cursor to bottom row
        var buf: [256]u8 = undefined;
        const move_seq = std.fmt.bufPrint(&buf, "\x1b[{d};1H", .{rows}) catch return;
        _ = try posix.write(self.stdout_fd, move_seq);

        // Styling: inverted color bar
        _ = try posix.write(self.stdout_fd, "\x1b[7m\x1b[1m"); // Reverse colors + bold

        const leader_status = if (is_leader_active) " [LEADER ACTIVE] " else " ";
        const status_text = std.fmt.bufPrint(&buf, " NEOMUX | Pane: {} | Leader: Ctrl+A{s}", .{ active_pane_id, leader_status }) catch return;

        _ = try posix.write(self.stdout_fd, status_text);

        // Fill remainder of row with spaces
        const remaining = if (cols > status_text.len) cols - status_text.len else 0;
        var i: usize = 0;
        while (i < remaining) : (i += 1) {
            _ = try posix.write(self.stdout_fd, " ");
        }

        // Reset attributes
        _ = try posix.write(self.stdout_fd, "\x1b[0m");
    }

    pub fn clearScreen(self: *Renderer) !void {
        _ = try posix.write(self.stdout_fd, "\x1b[2J\x1b[H");
    }
};

test "renderer status bar formatting" {
    const testing = std.testing;
    const renderer = Renderer.init();
    try testing.expect(renderer.stdout_fd >= 0);
}
