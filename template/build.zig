const std = @import("std");
const project = @import("project.zig");

pub fn build(b: *std.Build) !void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{ .preferred_optimize_mode = .Debug });
    const shared_sim_types = b.createModule(.{
        .root_source_file = b.path("../include/sim_types.zig"),
        .target = target,
        .optimize = optimize,
    });

    const mod = b.createModule(.{
        .root_source_file = b.path("src/root.zig"),
        .target = target,
        .optimize = optimize,
        .link_libc = true,
    });
    mod.addImport("shared_sim_types", shared_sim_types);

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
    const test_step = b.step("test", "Run template tests");
    test_step.dependOn(&run_tests.step);
}
