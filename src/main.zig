const std = @import("std");

pub fn main() !void {
    std.debug.print("Initializing Neomux (Ghostty-powered terminal multiplexer with Claude AI)\n", .{});
}

test "basic sanity test" {
    const value = 42;
    try std.testing.expectEqual(@as(i32, 42), value);
}
