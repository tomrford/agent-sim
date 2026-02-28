const std = @import("std");
const project = @import("project.zig");

pub fn build(b: *std.Build) !void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{ .preferred_optimize_mode = .Debug });

    const mod = b.createModule(.{
        .root_source_file = b.path("src/root.zig"),
        .target = target,
        .optimize = optimize,
        .link_libc = true,
    });

    for (project.include_paths) |p| {
        mod.addIncludePath(b.path(p));
    }

    const lib = b.addLibrary(.{
        .linkage = .dynamic,
        .name = project.name,
        .root_module = mod,
    });
    lib.linkLibC();
    b.installArtifact(lib);

    const tests = b.addTest(.{ .root_module = mod });
    const run_tests = b.addRunArtifact(tests);
    const test_step = b.step("test", "Run HVAC example tests");
    test_step.dependOn(&run_tests.step);
}
