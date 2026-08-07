const std = @import("std");
const posix = std.posix;
const c = std.c;

extern "c" fn forkpty(amaster: *c_int, name: ?[*]const u8, termp: ?*const anyopaque, winp: ?*const anyopaque) c_int;

pub const Winsize = extern struct {
    ws_row: u16 = 24,
    ws_col: u16 = 80,
    ws_xpixel: u16 = 0,
    ws_ypixel: u16 = 0,
};

pub const Pty = struct {
    master_fd: posix.fd_t,
    child_pid: posix.pid_t,
    allocator: std.mem.Allocator,

    pub fn spawn(allocator: std.mem.Allocator, shell_path: ?[]const u8, winsize: Winsize) !Pty {
        var master_fd: c_int = -1;
        const ws = winsize;

        const pid = forkpty(&master_fd, null, null, @ptrCast(&ws));
        if (pid < 0) {
            return error.ForkPtyFailed;
        }

        if (pid == 0) {
            // Child process: execute shell
            const default_shell = std.posix.getenv("SHELL") orelse "/bin/zsh";
            const target_shell = shell_path orelse default_shell;
            
            // Convert to null-terminated C string
            const shell_z = allocator.dupeZ(u8, target_shell) catch "/bin/zsh";
            
            const argv = [_]?[*c]const u8{
                shell_z.ptr,
                null,
            };

            const envp = [_]?[*c]const u8{
                "TERM=xterm-256color",
                null,
            };

            _ = c.execve(shell_z.ptr, @ptrCast(&argv), @ptrCast(&envp));
            c.exit(1);
        }

        // Parent process: return Pty handle
        return Pty{
            .master_fd = master_fd,
            .child_pid = @intCast(pid),
            .allocator = allocator,
        };
    }

    pub fn read(self: *Pty, buffer: []u8) !usize {
        return posix.read(self.master_fd, buffer);
    }

    pub fn write(self: *Pty, bytes: []const u8) !usize {
        return posix.write(self.master_fd, bytes);
    }

    pub fn resize(self: *Pty, winsize: Winsize) !void {
        const ws = winsize;
        const TIOCSWINSZ: u32 = 0x80087467; // Darwin ioctl TIOCSWINSZ
        _ = std.posix.system.ioctl(self.master_fd, TIOCSWINSZ, @intFromPtr(&ws));
    }

    pub fn deinit(self: *Pty) void {
        posix.close(self.master_fd);
        _ = posix.kill(self.child_pid, posix.SIG.TERM) catch {};
    }
};

test "spawn pty shell" {
    const testing = std.testing;
    var pty_inst = try Pty.spawn(testing.allocator, null, .{ .ws_row = 24, .ws_col = 80 });
    defer pty_inst.deinit();

    // Verify master fd is valid
    try testing.expect(pty_inst.master_fd >= 0);
    try testing.expect(pty_inst.child_pid > 0);
}
