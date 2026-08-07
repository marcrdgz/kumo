const std = @import("std");
const pane = @import("pane.zig");

pub const SplitDirection = enum {
    horizontal, // Top / Bottom
    vertical,   // Left / Right
};

pub const LayoutNode = union(enum) {
    leaf: *pane.Pane,
    split: struct {
        direction: SplitDirection,
        ratio: f32, // Split percentage (0.0 to 1.0)
        first: *LayoutNode,
        second: *LayoutNode,
    },

    pub fn createLeaf(allocator: std.mem.Allocator, target_pane: *pane.Pane) !*LayoutNode {
        const node = try allocator.create(LayoutNode);
        node.* = .{ .leaf = target_pane };
        return node;
    }

    pub fn destroy(self: *LayoutNode, allocator: std.mem.Allocator) void {
        switch (self.*) {
            .leaf => |target_pane| {
                target_pane.destroy();
            },
            .split => |s| {
                s.first.destroy(allocator);
                s.second.destroy(allocator);
            },
        }
        allocator.destroy(self);
    }
};

test "layout tree creation and split" {
    const testing = std.testing;
    const allocator = testing.allocator;

    const pane1 = try pane.Pane.create(allocator, 1, "Pane 1", .{ .ws_row = 24, .ws_col = 80 });
    const node1 = try LayoutNode.createLeaf(allocator, pane1);

    const pane2 = try pane.Pane.create(allocator, 2, "Pane 2", .{ .ws_row = 24, .ws_col = 80 });
    const node2 = try LayoutNode.createLeaf(allocator, pane2);

    const split_node = try allocator.create(LayoutNode);
    split_node.* = .{
        .split = .{
            .direction = .vertical,
            .ratio = 0.5,
            .first = node1,
            .second = node2,
        },
    };
    defer split_node.destroy(allocator);

    // Verify split direction
    switch (split_node.*) {
        .split => |s| {
            try testing.expectEqual(SplitDirection.vertical, s.direction);
            try testing.expectEqual(@as(f32, 0.5), s.ratio);
        },
        .leaf => return error.TestUnexpectedLeaf,
    }
}
