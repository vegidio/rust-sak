# `ai` module

Installs what ONNX inference needs (the ONNX Runtime, and NVIDIA's CUDA, cuDNN and TensorRT on machines whose GPU can use them) from **Hugging Face folders the caller names**, verifies every download against the **hash the Hugging Face API reports**, and loads the runtime once per process.

Nothing in this module names a dependency, a version, a folder, a size, a hash or a place on disk. The caller passes each dependency as its folder URL, such as `https://huggingface.co/vegidio/general/tree/main/deps/cuda/13.3.0`, and the root folder to install under. A new version on Hugging Face needs only a new URL in the calling app.

## Enabling

The module is gated behind the `ai` Cargo feature, which turns on `crypto`, `fetch`, `fs` and `sysinfo` as well:

```toml
[dependencies]
rust-sak = { git = "https://github.com/vegidio/rust-sak", features = ["ai"] }
```

`install` is async and needs a **Tokio** runtime, as `fetch` does. `load` and the rest are synchronous.

### The `ort` pin

`ai` builds **`ort = "=2.0.0-rc.15"`** with no default features and `std`, `load-dynamic`, `api-30` and `tracing`. A consumer that builds sessions with `ort` itself must ask for that same version and API level, so that one `ort` is compiled and it accepts the 1.30 runtime it loads. `download-binaries` and `copy-dylibs` stay off: the runtime comes from Hugging Face at run time, not from the build.

Linking `ai` sets `ORT_DISABLE_TELEMETRY=1` before `main`, which is the only full opt-out of ONNX Runtime's telemetry uploader on macOS and Linux.

## Public API

| Item                                  | What it does                                                                                                                                                                                                                         |
|---------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `Source::parse(url)`                  | Parses a Hugging Face folder URL (`https://huggingface.co/<owner>/<repo>/tree/<revision>/<folder>`) into its repo, revision and folder. The folder's last two segments are the dependency's name and version.                        |
| `Dependency::runtime(name, source)`   | The ONNX Runtime. Installed only while its library is in its folder, and a platform its folder has no archive for fails the install.                                                                                                |
| `Dependency::libraries(name, source)` | Libraries no one file stands for, such as CUDA. Its marker alone decides, and a platform its folder has no archive for is skipped.                                                                                                   |
| `install(root, &deps, on_event)`      | Installs each dependency that is missing or out of date into `dirs::dir_for(root, source)`, in order, reporting `Event`s. Fails with an `InstallError` naming the dependency.                                                        |
| `is_present(root, &dep)`              | Local only: whether a dependency's marker (and, for the runtime, its library) is there. For deciding whether to load, with no network.                                                                                              |
| `load(runtime_dir)`                   | Loads the runtime installed in `runtime_dir` and starts the process's environment, its logs going to `tracing` at warning and above. Once per process: a later call returns at once. A failed load can be tried again.             |
| `runtime_library(dir)`                | The runtime library in `dir`: `libonnxruntime*.dylib`, `libonnxruntime.so*` or `onnxruntime.dll`, never a `*_providers_*` plugin.                                                                                                   |
| `asset_for(listing, os, arch)`        | The one `*_<macos\|linux\|windows>_<arm64\|x64>.7z` in a folder's listing, for `std::env::consts` spellings.                                                                                                                        |
| `dirs::dir_for(root, source)`         | `<root>/<name>/<version>`.                                                                                                                                                                                                           |
| `gpu::meets(&GpuRequirement)`         | Whether this machine has an NVIDIA GPU whose driver supports at least `min_driver` (CUDA version) and whose first CUDA device has at least `min_compute` (compute capability). A probe that fails counts as no GPU, with a warning. |
| `gpu::meets_with(gpus, cuda, req)`    | The same comparison over probe results the caller already has. Pure.                                                                                                                                                                |
| `hf::Hub`                             | A Hugging Face client for one repository: `resolve(revision)` gives the commit it points at, `list(commit, folder)` gives each file's `size` and `hash`, and `url(commit, path)` builds the `resolve` URL.                          |
| `hf::Hash`                            | `Sha256` (the LFS hash) or `GitBlobSha1` (the object id of a file kept outside LFS), with `verify_file(path)`.                                                                                                                       |

### Events and errors

`on_event` receives `Event::Progress { dependency, fraction }` and `Event::Verifying { dependency }`. A dependency's fraction runs from 0 to 1: the download fills 0 to 0.8 (a resumed partial counted from the first report), and the extraction fills 0.8 to 1. It never decreases, and is exactly 1 only once the marker is written. A dependency that is already current reports nothing.

`InstallError { dependency, failure }` displays as `could not install NVIDIA cuDNN: the server answered with HTTP 404`. `Failure` is one of `Network`, `Http(status)`, `DiskFull`, `Io(reason)`, `Damaged`, `UnsupportedPlatform { os, arch }` and `Malformed(reason)`.

## How an install works

For each dependency, in the order given:

1. The source's revision (`main`) is resolved to a commit, once per repository per call. Everything after uses that commit, so a file re-uploaded mid-install cannot look damaged.
2. The folder is listed at that commit, and the archive for this platform is picked from the listing.
3. The dependency is **current** when its marker holds the hash the API reports now (and, for the runtime, its library is a file). It is skipped, with no event.
4. Otherwise the archive is downloaded into `.staging/` from `resolve/<commit>/<path>`, resuming a partial whose hash is still the API's, even after `main` moved to a new commit. It is verified against the API's hash, extracted in staging with every entry treated as untrusted, and moved into place: the marker deleted first, every other entry but staging removed, the extracted entries moved in, and the marker written last.

When the API cannot be reached (no network, a timeout, or a 5xx after the retries), a dependency that `is_present` is used as it is, with a warning. Only a missing install needs the network. Every request retries transient failures 5 times with Fibonacci backoff, and times out after 30 seconds of connecting or of silence.

Dropping the `install` future stops it. A download keeps its partial to resume from. An extraction that has begun runs to its end on its blocking thread, so a marker never stands over an incomplete install.

### On disk

```text
<root>/<name>/<version>/                     installed: the archive's entries, at the paths it gives them
<root>/<name>/<version>/.installed           the archive's verified hash: written last, deleted first
<root>/<name>/<version>/.staging/<asset>     the archive (and the download's .part and sidecar while it downloads)
<root>/<name>/<version>/.staging/extract/    this attempt's extraction
```

Apps that pass the same root share the installs, and different versions of a dependency live side by side.

### One version per folder

Every app that installs from a folder accepts whatever archive the API reports there, including an archive re-uploaded after the app was released. **A version folder on Hugging Face must only ever hold that version.** Putting 1.31 bytes in `deps/onnx/1.30.0/` would break every released app on its next start. Publish a new version in a new folder, and point new builds at its URL.

The hash comes from the same host as the file, so verification catches corruption, truncation and a re-upload during a resume, not a compromised repository. Integrity rests on HTTPS to Hugging Face and on control of the repository.

## Usage

```rust,no_run
use std::path::Path;

use rust_sak::ai::{self, Dependency, Event, Source, gpu::GpuRequirement};

const GPU: GpuRequirement = GpuRequirement { min_driver: (13, 0), min_compute: (7, 5) };

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let folder = |path: &str| Source::parse(&format!("https://huggingface.co/vegidio/general/tree/main/{path}"));
let runtime = folder("deps/onnx/1.30.0")?;

let mut dependencies = vec![Dependency::runtime("ONNX Runtime", runtime.clone())];
if ai::gpu::meets(&GPU) {
    dependencies.push(Dependency::libraries("NVIDIA CUDA", folder("deps/cuda/13.3.0")?));
    dependencies.push(Dependency::libraries("NVIDIA cuDNN", folder("deps/cudnn/9.23.1")?));
    dependencies.push(Dependency::libraries("NVIDIA TensorRT", folder("deps/tensorrt/10.14.1")?));
}

let root = Path::new("/path/the/app/chose/io.vinicius.ai");
ai::install(root, &dependencies, |event| {
    if let Event::Progress { dependency, fraction } = event {
        println!("{dependency}: {:.0}%", fraction * 100.0);
    }
})
.await?;

ai::load(&ai::dirs::dir_for(root, &runtime))?;
# Ok(())
# }
```

The Hugging Face client works on its own too:

```rust,no_run
use rust_sak::ai::hf::Hub;

# async fn run() -> Result<(), rust_sak::ai::hf::HfError> {
let hub = Hub::new("vegidio/general");
let commit = hub.resolve("main").await?;
for file in hub.list(&commit, "siglip2").await? {
    println!("{} {} bytes, {:?}, from {}", file.path, file.size, file.hash, hub.url(&commit, &file.path));
}
# Ok(())
# }
```

## Platform notes

- The archives exist for macOS arm64, Linux arm64/x64 and Windows arm64/x64 (the runtime), and Linux arm64/x64 and Windows x64 (the NVIDIA libraries). Intel Macs have no runtime archive. Finding that out takes the folder's listing, so the unsupported-platform failure comes after one API request.
- **Only macOS is tested.** The GPU paths (the NVIDIA probe passing, and the CUDA, cuDNN and TensorRT installs) have never run on real hardware. Their logic is covered by pure tests against constructed probe results, and the platform-specific code is written to be correct by construction, but it is **untested on Linux and Windows**.
- Installing the NVIDIA libraries does not make them findable by the loader (`LD_LIBRARY_PATH` on Linux, `PATH` on Windows). That comes with sessions, in a later release.
