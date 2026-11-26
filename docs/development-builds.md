# Windows development builds

Use `cargo check --locked` for compiler feedback, a focused `cargo test --locked <name>` for behavior, and `.\build.ps1` when you need the development executable. Both the development and test profiles reuse incremental compiler state for workspace code.

## Reuse one dependency build

For sequential worktrees, point Cargo's intermediate build directory at an already warm target. Cargo can then reuse dependencies while each worktree keeps its own output directory. In a normal Git repository with linked worktrees, the following PowerShell session uses intermediates from the main checkout. If an existing campaign has a different warm directory, use its absolute path here.

~~~powershell
$commonGit = git rev-parse --path-format=absolute --git-common-dir
$env:CARGO_BUILD_BUILD_DIR = Join-Path (Split-Path -Parent $commonGit) 'target'
cargo check --locked
cargo test --locked browser::tree
.\build.ps1
~~~

Use `cargo run --locked` in the intended checkout. For a build while its player is running, pass `--target-dir <separate-output-directory>` with the same intermediate directory. Windows locks a running executable. A shared `CARGO_TARGET_DIR` also works, but then all worktrees share the top-level executable as well. Cargo serializes writers to a shared build directory. Schedule builds in sequence or give concurrent jobs independent build directories and a CPU budget. [Cargo build and output directories](https://doc.rust-lang.org/cargo/reference/config.html#buildbuild-dir)

Keep the toolchain, target triple, feature selection and compiler flags consistent. Changing these inputs can invalidate dependency artifacts. A workspace edit normally invalidates its own crate and downstream users. A new worktree can reuse matching registry and Git dependencies even when its local crate and build script need compilation. Preserve the target between iterations.

## Choose the build for the question

| Work | Command | Output |
| --- | --- | --- |
| Types and borrow checking | `cargo check --locked` | Compiler diagnostics, without the player link |
| Normal interaction work | `.\build.ps1` | Incremental development player |
| Exercise optimized Disqro code | `.\build.ps1 -Optimized` | App compiled at optimization level 1 in the dev target |
| Runtime performance or delivery | `.\build.ps1 -Release` | Production release profile |
| Standalone engine work | `cargo test --locked -p waveform-core` in the engine checkout | Core tests without the GPUI viewer |

The optimized iteration command applies Cargo's package-specific dev override to Disqro. It reuses the normal dependency graph and writes the dev executable. Production performance measurements use release optimization for the entire graph. Cargo reads profile settings from the workspace root. Disqro controls its Git dependencies' profiles, while standalone Waveform Native controls its workspace builds. [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)

## Measurements

Windows x64, Rust 1.99.1. The Disqro comparison used the PR's starting revision in one isolated checkout and the same warm target. Each profile built a baseline, changed the same diagnostic string in the same module, then built the identical source again. Runs were serial. The table gives Cargo's elapsed time including preparation. A seed builds the local crate and initializes that configuration. Dependencies were already available for the Disqro seeds.

| Build | Seed | One-line edit | No changes |
| --- | ---: | ---: | ---: |
| Disqro dev, previous settings | 25.34 s | 21.75 s | 0.74 s |
| Disqro dev, incremental | 25.23 s | 11.34 s | 0.75 s |
| Disqro dev, incremental, app at level 1 | 53.07 s | 26.55 s | 0.73 s |
| Standalone waveform-core, previous settings | 9.20 s | 3.14 s | 0.20 s |
| Standalone waveform-core, incremental | 4.13 s | 1.25 s | 0.22 s |

Each Disqro edit rebuilt one binary unit and reused 593 units. Each no-change build reused all 594. The first build from the isolated checkout reused 591 units and built three. The engine edits each reused 26 of 27 units. The first engine seed built 22 units, so the engine seed times include different amounts of dependency work. Measurements are single samples on this machine.

The reported 7 min 29 s release matches a worktree-local dependency build. That target contained 576 compiler invocation timestamps spanning 447.84 s from the first crate to the release executable. The Disqro binary unit began about 80.6 s before the executable was written. These filesystem times corroborate a cold target and do not measure downloads or individual compiler phases. With these intermediates, a production release took 73.7 s on the first corrected source and 71.9 s after the rescan correction. Each run reused 637 units and compiled the Disqro binary. Windows denied replacement of the running player during an earlier attempt. Both successful builds wrote a separate executable.

For the engine library edit, Cargo separated front-end time (1.16 s before, 0.54 s incremental) and code generation (1.77 s before, 0.47 s incremental). This library build produced an rlib.

Use `cargo build --locked --timings` to examine the unit graph, fresh units, and dirty units. Cargo writes a timestamped report in `cargo-timings` below the active target. The Disqro binary unit has no separate code-generation or linking sections in this toolchain's HTML. During the controlled edits, 100 ms polling observed the Windows linker in 27, 26 and 45 samples respectively for previous dev, incremental dev and app-optimized dev. These are approximate linker occupancy observations. Front-end work, code generation, incremental serialization and other overhead share the remaining unit time. [Cargo timing reports](https://doc.rust-lang.org/cargo/reference/timings.html)

The shared target already supplied every dependency on each measured edit. The remaining Disqro unit invokes the system linker, which sccache excludes. An sccache benchmark would be relevant for a workload with frequent cold targets and repeated cacheable library compilations. No speedup from sccache is assumed in these measurements. [sccache Rust support](https://github.com/mozilla/sccache/blob/main/docs/Rust.md)
