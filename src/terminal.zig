const std = @import("std");
const posix = std.posix;
const pty = @import("pty.zig");

pub const Terminal = struct {
    orig_termios: posix.termios,
    tty_fd: posix.fd_t,
    stdout_fd: posix.fd_t,

    pub fn enableRawMode() !Terminal {
        // Open /dev/tty to ensure access to controlling terminal even if stdin/stdout are redirected
        const tty_fd = posix.open("/dev/tty", .{ .ACCMODE = .RDWR }, 0) catch posix.STDIN_FILENO;
        const stdout_fd = posix.STDOUT_FILENO;

        const orig_termios = posix.tcgetattr(tty_fd) catch |err| {
            if (tty_fd != posix.STDIN_FILENO) posix.close(tty_fd);
            return err;
        };

        var raw = orig_termios;

        // Disable echo, canonical mode, extended input, signals
        raw.lflag.ECHO = false;
        raw.lflag.ICANON = false;
        raw.lflag.IEXTEN = false;
        raw.lflag.ISIG = false;

        // Disable software flow control & CR to NL mapping
        raw.iflag.IXON = false;
        raw.iflag.ICRNL = false;
        raw.iflag.BRKINT = false;
        raw.iflag.INPCK = false;
        raw.iflag.ISTRIP = false;

        // Disable output processing
        raw.oflag.OPOST = false;

        // Character size 8 bits
        raw.cflag.CSIZE = .CS8;

        // Read timeout & min characters
        raw.cc[@intFromEnum(posix.V.MIN)] = 1;
        raw.cc[@intFromEnum(posix.V.TIME)] = 0;

        posix.tcsetattr(tty_fd, .FLUSH, raw) catch |err| {
            if (tty_fd != posix.STDIN_FILENO) posix.close(tty_fd);
            return err;
        };

        // Enter alternate screen buffer & clear screen
        _ = posix.write(stdout_fd, "\x1b[?1049h\x1b[2J\x1b[H") catch {};

        return Terminal{
            .orig_termios = orig_termios,
            .tty_fd = tty_fd,
            .stdout_fd = stdout_fd,
        };
    }

    pub fn disableRawMode(self: *Terminal) void {
        // Leave alternate screen buffer & show cursor
        _ = posix.write(self.stdout_fd, "\x1b[?1049l\x1b[?25h") catch {};
        posix.tcsetattr(self.tty_fd, .FLUSH, self.orig_termios) catch {};
        if (self.tty_fd != posix.STDIN_FILENO) {
            posix.close(self.tty_fd);
        }
    }

    pub fn getWindowSize(self: ?*const Terminal) struct { rows: u16, cols: u16 } {
        var ws: pty.Winsize = undefined;
        const target_fd = if (self) |t| t.tty_fd else posix.STDOUT_FILENO;
        const TIOCGWINSZ: u32 = 0x40087468; // Darwin TIOCGWINSZ
        const rc = std.posix.system.ioctl(target_fd, TIOCGWINSZ, @intFromPtr(&ws));
        if (rc != 0 or ws.ws_col == 0) {
            return .{ .rows = 24, .cols = 80 };
        }
        return .{ .rows = ws.ws_row, .cols = ws.ws_col };
    }
};

test "terminal window size" {
    const size = Terminal.getWindowSize(null);
    try std.testing.expect(size.rows > 0);
    try std.testing.expect(size.cols > 0);
}
