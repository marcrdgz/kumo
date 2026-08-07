const std = @import("std");
const posix = std.posix;
const terminal = @import("terminal.zig");
const renderer = @import("renderer.zig");
const pty = @import("pty.zig");
const pane = @import("pane.zig");
const layout = @import("layout.zig");

pub const App = struct {
    allocator: std.mem.Allocator,
    term: terminal.Terminal,
    rend: renderer.Renderer,
    root_node: *layout.LayoutNode,
    active_pane: *pane.Pane,
    is_running: bool,
    is_leader_active: bool,

    pub fn init(allocator: std.mem.Allocator) !*App {
        var term_inst = try terminal.Terminal.enableRawMode();
        errdefer term_inst.disableRawMode();

        const win_size = term_inst.getWindowSize();

        const initial_pane = try pane.Pane.create(
            allocator,
            1,
            "Main Shell",
            .{ .ws_row = win_size.rows - 1, .ws_col = win_size.cols },
        );
        initial_pane.is_focused = true;

        const root_node = try layout.LayoutNode.createLeaf(allocator, initial_pane);

        const app_ptr = try allocator.create(App);
        app_ptr.* = .{
            .allocator = allocator,
            .term = term_inst,
            .rend = renderer.Renderer.init(),
            .root_node = root_node,
            .active_pane = initial_pane,
            .is_running = true,
            .is_leader_active = false,
        };

        return app_ptr;
    }

    pub fn run(self: *App) !void {
        defer self.term.disableRawMode();

        var read_buf: [1024]u8 = undefined;

        while (self.is_running) {
            const win_size = self.term.getWindowSize();
            try self.rend.drawStatusBar(win_size.rows, win_size.cols, self.active_pane.id, self.is_leader_active);

            // Poll tty_fd and PTY master fd
            var poll_fds = [_]posix.pollfd{
                .{ .fd = self.term.tty_fd, .events = posix.POLL.IN, .revents = 0 },
                .{ .fd = self.active_pane.pty_process.master_fd, .events = posix.POLL.IN, .revents = 0 },
            };

            const poll_res = posix.poll(&poll_fds, 100) catch continue;
            if (poll_res == 0) continue;

            // Handle user keyboard input
            if (poll_fds[0].revents & posix.POLL.IN != 0) {
                const nread = posix.read(self.term.tty_fd, &read_buf) catch 0;
                if (nread > 0) {
                    for (read_buf[0..nread]) |char| {
                        try self.handleCharInput(char);
                    }
                }
            }

            // Handle PTY shell output
            if (poll_fds[1].revents & posix.POLL.IN != 0) {
                const nread = self.active_pane.pty_process.read(&read_buf) catch 0;
                if (nread > 0) {
                    _ = try posix.write(self.term.stdout_fd, read_buf[0..nread]);
                }
            }
        }
    }

    fn handleCharInput(self: *App, char: u8) !void {
        const LEADER_KEY: u8 = 0x01; // Ctrl+A

        if (self.is_leader_active) {
            self.is_leader_active = false;
            switch (char) {
                'q', 'd', 0x03 => { // 'q', 'd' or Ctrl+C after leader -> Quit
                    self.is_running = false;
                },
                'c' => {
                    // Claude AI pane placeholder action
                    _ = posix.write(self.term.stdout_fd, "\r\n[Neomux] Invoking Claude AI Assistant...\r\n") catch {};
                },
                else => {},
            }
            return;
        }

        if (char == LEADER_KEY) {
            self.is_leader_active = true;
            return;
        }

        // Pass normal character to active PTY master process
        const input_slice = [_]u8{char};
        _ = try self.active_pane.pty_process.write(&input_slice);
    }

    pub fn deinit(self: *App) void {
        self.root_node.destroy(self.allocator);
        self.allocator.destroy(self);
    }
};
