const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    // Main Executable Module
    const exe_mod = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });

    const exe = b.addExecutable(.{
        .name = "neomux",
        .root_module = exe_mod,
    });
    exe.linkLibC();
    exe.linkFramework("AppKit");
    exe.linkFramework("Cocoa");
    exe.linkFramework("QuartzCore");
    exe.linkFramework("Metal");

    b.installArtifact(exe);

    // `zig build run`
    const run_cmd = b.addRunArtifact(exe);
    run_cmd.step.dependOn(b.getInstallStep());

    if (b.args) |args| {
        run_cmd.addArgs(args);
    }

    const run_step = b.step("run", "Run the Neomux terminal multiplexer GUI");
    run_step.dependOn(&run_cmd.step);

    // `zig build test`
    const exe_tests = b.addTest(.{
        .root_module = exe_mod,
    });
    exe_tests.linkLibC();
    exe_tests.linkFramework("AppKit");
    exe_tests.linkFramework("Cocoa");
    exe_tests.linkFramework("QuartzCore");
    exe_tests.linkFramework("Metal");

    const run_exe_tests = b.addRunArtifact(exe_tests);
    const test_step = b.step("test", "Run unit tests");
    test_step.dependOn(&run_exe_tests.step);

    // `zig build check` (Fast compile check without codegen)
    const check_exe = b.addExecutable(.{
        .name = "neomux-check",
        .root_module = exe_mod,
    });
    check_exe.linkLibC();
    check_exe.linkFramework("AppKit");
    check_exe.linkFramework("Cocoa");
    check_exe.linkFramework("QuartzCore");
    check_exe.linkFramework("Metal");

    const check_step = b.step("check", "Check if code compiles without emitting binary");
    check_step.dependOn(&check_exe.step);
}
