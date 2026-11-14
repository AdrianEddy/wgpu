# Branch `trunk-oxivideo-fixes`

This branch is the owner's wgpu base: upstream `trunk`, plus the fixes the owner's products need
that `trunk` lacks. This file is not meant for upstream; every other commit is, one PR per fix or per
group named below.

It replaces the vendored copy at `face-masks-rs/vendor/wgpu` (v30.0.1 plus local changes),
whose `VENDORED.md` §1 (lazy adapter selection, gfx-rs/wgpu#10011), §2, §3, §5 and §6 the commits
below port, with §4's tests where wgpu keeps such tests.

## Base

| | Commit | |
|---|---|---|
| Base | `60cef2be8` | upstream `trunk`, 2026-11-25 07:50 UTC, "Implement DXGI Swapchain on Vulkan (#8388)" |

gfx-rs/wgpu#10675, which the branch carried as a merge before this base, is in it (`64bd5d5ab`).

The table gives each commit as it is on this base. A SHA in the text that is not on this branch is a
commit of the branch before its rebase onto `60cef2be8` (whose last commit was `525473bb1`): what the
text says ran there ran on that commit.

## Carried commits

Every commit type-checks on its own (`cargo check`, each crate with its default features; `deno_webgpu` and `cts_runner`
left out, as CI builds them apart): on Linux the workspace with every target, wgpu-hal with the test hooks it has, CI's
two wasm32 rows and the Android libraries; on Windows the workspace, wgpu-hal with the test hooks it has, and the macOS
and iOS libraries; and from Linux the macOS, iOS and Windows libraries, type-checked. The CPU and GPU suites ran at the
tip. Each fix has a test that fails without it and passes with it, except where the table says otherwise. Each fix
commit has a `CHANGELOG.md` entry in `trunk`'s format, without a PR link where it has no PR yet: add the link when one
is opened. Commit 35's two entries link gfx-rs/wgpu#10011, which upstream closed unmerged. 39 refines 35's change, which
no release has, so it has no entry of its own.

| # | Commit | Subject | Fixes | Drop when |
|---|---|---|---|---|
| 1 | `a9783ffd5` | fix(core)!: report a lost device from `poll` as `PollError::DeviceLost` | `VENDORED.md` §6: a poll that finds the device lost panicked in `wgpu::Device::poll`. Adds `PollError::DeviceLost`, a breaking change: `PollError` is exhaustive. | a release contains it |
| 2 | `2f2214ecc` | fix(core): set the bind groups after a group that becomes unused again | Found in review: a pipeline change that left a group without a layout did not set the later groups again, so GL and DX12 used another group's resources, and Vulkan's later sets were incompatible. GPU test, failing on every adapter without it. | a release contains it |
| 3 | `ba318ac80` | fix(gles): number a pipeline layout's bindings per pass kind | §5: a layout within the per-stage limits that holds more than 16 textures or samplers in all its stages panicked in `set_bind_group`. Render and compute passes number their bindings apart, and a binding that no stage of a pass can see takes no slot. Caps the per-stage buffer and storage texture limits at 128. | a release contains it |
| 6 | `73ca0df12` | fix(wgpu): unmap a destroyed device's buffers with their set unlocked | Found in review: `Device::destroy` deadlocked on its own lock when another thread dropped a buffer meanwhile. | a release contains it |
| 7 | `b240c038f` | fix(vulkan): give a discarded surface texture's image back | New: Vulkan's discard did nothing (gfx-rs/wgpu#5723, no open PR). The swapchain keeps the discarded texture, with its semaphores, and the next acquisition returns it, as suboptimal as it was acquired, once the submissions that used it have finished. Tested only by commit 26's `discarded_images`, which needs a window. | a release contains it |
| 8 | `100d8fdea` | fix(noop): report the device type that `NoopBackendOptions` asks for | New: the option was ignored. `None` keeps `DeviceType::Cpu`, and the documentation now says so (it said `Other`). | a release contains it |
| 9 | `927418044` | feat(noop): add surfaces that keep a swapchain's books | Test infrastructure: noop surfaces that configure, acquire, present and discard. An enabled noop adapter now satisfies `compatible_surface`. Changes noop's `Api` types. | upstream takes it, or the tests below are dropped |
| 10 | `c8cf29886` | feat(noop): check the HAL's surface requirements on noop surfaces | Test infrastructure: records each broken requirement of `wgpu_hal::Surface` and `Queue` as a `SurfaceViolation`. | as commit 9 |
| 11 | `044fbb6fb` | feat(noop): add test hooks to noop surfaces, devices and queues | Test infrastructure behind wgpu-hal's non-default `noop_test_hooks` feature, which `tests/Cargo.toml` enables. | as commit 9 |
| 12 | `a541a0274` | fix(core): report a GPU wait that times out while configuring a surface | GL's bounded waits made configure, unconfigure and drop panic. Configure returns `GpuWaitTimeout` and keeps its configuration; the acquired texture is given back to the HAL before the wait. | a release contains it |
| 13 | `e66ffcaf3` | fix: drop replaced handlers and devices with no lock held | Found in review: replacing the uncaptured error handler or the device lost callback, or configuring a surface for another device, dropped the application's state under a lock. | a release contains it |
| 14 | `b5ab34b64` | fix(core)!: call the application's callbacks only when no lock is held | First review item 4 and VENDORED §2's callback work, as one mechanism: callbacks are queued on the instance and delivered when the outermost `CallbackScope` on the thread closes. The uncaptured error handler runs outside the scopes. `Device::get_queue` is no longer public; `Device::queue` gives a caller the queue as a reference of its own, for gfx-rs/wgpu#10895's `wgpu::Device::queue` (`688afecd0`, in the base). | a release contains it |
| 15 | `23838ead0` | fix(core): release a failed submission's locks before its work drops | First review item 2: a failed submission dropped its unsubmitted work under `pending_writes` or `trackers`, and `pre_submit`'s error path released its guards out of order. | a release contains it |
| 16 | `b73deed69` | fix(core): deliver callbacks after errors and in order | Completes commit 14: every error report delivers, nested deliveries keep the order, and an empty queue takes no lock. Its test of concurrent deliveries is built only with `send_sync`, where callbacks are `Send`, as CI's wasm32 clippy row needs (the owner's decision 4 of review 4). | with commit 14 |
| 17 | `4ec8dc4c7` | fix(core): check a presented texture's init state under pending writes | Found in review: a presentation could clear a texture that another thread wrote meanwhile. | a release contains it |
| 18 | `198061ebc` | fix(core): destroy the views of surface textures before (un)configuring | §2: views of surface textures the application still holds (gfx-rs/wgpu#4105). A release while unwinding never waits for the snatch lock. Documents that an `as_hal` guard must not be held across surface calls. | a release contains it |
| 19 | `2e253c5b7` | fix(wgpu): log the error of a surface texture released while unwinding | The release reported its error to the default handler, which panicked while unwinding, so the process aborted. | a release contains it |
| 20 | `4e0c3f0bd` | fix(core): acquire surface textures under the presentation lock | §2: acquire under the lock, `AlreadyAcquired` checked before the HAL acquire, clear-view failure gives the texture back. | a release contains it |
| 21 | `90ad710f1` | fix(core): submit a write to a surface texture at once | First review item 3: a write to a surface texture is submitted before `write_texture` returns, so no destruction comes between. Each such write costs a submission. | a release contains it |
| 22 | `6deccc80a` | fix(core)!: keep a surface texture acquired until the HAL presents it | §2: present holds the surface's lock up to the HAL present; a failed preparation gives the texture back. New HAL contract: configure and unconfigure are synchronized with acquire, discard and present. | a release contains it |
| 23 | `c2db787de` | fix(core): return a surface texture's image on every path that ends it | Found in review: `Texture::destroy` of an acquired surface texture, and a release while unwinding, left its image acquired. | a release contains it |
| 24 | `e05e723c9` | fix(metal): honour the timeout of a surface texture acquisition | `nextDrawable` waited indefinitely although wgpu-core asks for one second. Type-checked for `aarch64-apple-ios` only, not run. | a release contains it |
| 25 | `207d190c6` | docs: document the synchronization that surface configuration relies on | The documentation that goes with gfx-rs/wgpu#10675, which is in the base (`64bd5d5ab`) without it, and corrections of `wgpu::Surface`'s. For upstream (the owner's ruling, 2026-11-27), its #10675 half (wgpu-hal's `Surface::configure`/`unconfigure` and `Queue::submit` contracts, and DX12's and Vulkan's comments) is a docs follow-up to #10675, and its `wgpu::Surface` corrections, which describe `trunk`'s own behaviour, a docs PR of their own: both cut on `trunk`, since here their hunks sit on text of 12, 18 and 22. | a release contains it |
| 26 | `cdc4f54c2` | test: add window tests of surface operations on real backends | Window-only cases on real backends. Built, not run. | with commits 7, 18 to 23 |
| 27 | `edad2cf63` | test: name the adapter, phase and round in the window tests' failures | Third review item 5: every window trial prints its adapters and fails if it ran on none, `WGPU_SURFACE_TEST_ADAPTER` limits the trials to some adapters, a lost device counts as an error, and a failure names the adapter, phase and round, with the errors and validation messages collected so far. Test file only. | with commit 26 |
| 28 | `07e440bf0` | fix(core)!: wait for submissions before the queue's idle waits | First review item 1, third review items 2 and 3: dropping the queue and tearing a surface configuration down wait on the fence for wgpu's submissions with no lock held, then check under `command_indices` (`lock_after_submissions`: three tries, then the last wait under the lock). In `Queue::drop`, a wait that finds the device lost skips the idle wait, and any other error aborts (panics without `std`), as gfx-rs/wgpu#9427 made the idle wait's errors do: the GPU may still use what the drop frees. This goes one step past #9427, which has no fence wait: a fence wait's other error aborts too, since the idle wait that would follow requires every submission complete (`Queue::wait_for_idle`'s safety section), and losing the device instead would let `maintain` free what the GPU may still use. A failed wait before a surface teardown loses the device. Breaking HAL contract: `Queue::wait_for_idle` and the teardown in `Surface::configure`/`unconfigure` require every submission complete, or a wait without timeout that gave up (GL, WebGL). | a release contains it |
| 29 | `7b14efb5d` | fix(vulkan): wait for the presenting queue in swapchain teardown | First review item 1, third review item 1: `vkQueueWaitIdle` on the swapchain's own queue, which covers its presents (Vulkan 1.4.324), instead of `vkDeviceWaitIdle`, which needs every queue of the device externally synchronized. The DXGI swapchain of the Vulkan backend (gfx-rs/wgpu#8388, the base) waited for the device too: it now waits for the device's queue, under the queue lock from 30 on. The window gate ran (2026-11-12/13; Tests not run here) and added no failure; resizes through this wait, the fallback from 33 on, are slow on AMD's integrated GPUs (Residual behaviour). | a release contains it |
| 30 | `3362b4c1b` | feat(vulkan): expose the lock wgpu-hal takes around queue commands | First review item 1: wgpu-hal takes a `QueueLock` around exactly its queue commands and hands it out (`Queue::queue_lock`, `lock_queue`, also on `Device`), for code that submits to wgpu's `VkQueue`. Its own lock is a binary semaphore over `wgpu_sync`'s mutex and condition variable, with no lock_api guard state (third review item 8). | upstream carries an equivalent |
| 31 | `376179016` | feat(vulkan)!: support queues created internally synchronized | Third review items 6 and 7: `device_from_raw` takes a `DeviceQueue` (`#[non_exhaustive]`, `new` and setters; breaking). A queue created with `VK_DEVICE_QUEUE_CREATE_INTERNALLY_SYNCHRONIZED_BIT_KHR` is retrieved with `vkGetDeviceQueue2` and takes no lock; the extension, `vkGetDeviceQueue2` and a non-null queue are checked. Nothing opts in automatically. | upstream carries an equivalent |
| 32 | `1b57b927d` | feat(vulkan): take the application's queue lock, with scopes and checks | Fork-only extensions (third review items 4 and 6): `QueueLockScope`, an application lock given in `DeviceQueue::lock` and always taken, debug tracking by the lock's identity, the staging methods' asserts, and wgpu-core's `ranked.rs` check that no queue lock is held when it takes a lock (lock validation). | the consumers need neither scopes nor injection, or upstream takes them |
| 33 | `fdb2260c0` | feat(vulkan): wait on present fences in swapchain teardown | Owner's decision 1 of the third fix round: presents signal fences with `swapchain_maintenance1` (KHR, else EXT, per device), and the teardown waits on them without the queue lock; `vkQueueWaitIdle` under the lock stays the fallback. `Instance::desired_extensions` adds the surface maintenance extensions. | upstream carries an equivalent |
| 34 | `f5bf6ea78` | docs: add FORK.md | This file | the branch is dropped |
| 35 | `88c5dbf4d` | feat(core,dx12)!: select adapters lazily in `request_adapter` | `VENDORED.md` §1, gfx-rs/wgpu#10011: `request_adapter` exposed every adapter to rank them, and DX12 creates a device for each, seconds per adapter (OxQuick measured 2.7 s per start for a Radeon iGPU that then lost the selection). A backend may now select its adapter without exposing the others. DX12 orders DXGI's adapters and exposes them in that order until one exposes, so it creates devices only up to its selection: a probe of OxQuick's DX12 start here went from 2.2 s to 0.26 s. Ported onto `trunk`'s `request_adapters` with one filter and one ranking. The DX12 device test fails at the base. | gfx-rs/wgpu#10775 is released and filters the enumeration that `request_adapter` uses: then only the DX12 selection and its HAL hook go, and OxQuick selects by LUID ("The future of lazy adapter selection", below) |
| 36 | `78c42e9e0` | test(noop): select the noop adapter lazily and count adapter queries | Test infrastructure for 35: instance hooks behind `noop_test_hooks`, with which wgpu-core's lazy selection is tested without a GPU, also for a request with a surface. Each of its seven `lazy_adapter` tests fails with one of four mutations of 35; its four HAL tests check the hooks themselves, among them that a fallback request selects no hardware adapter. Upstream, it needs commits 9 and 11. | with 35's DX12 selection |
| 37 | `2581eb3d6` | test: request adapters in a browser whose WebGPU has no adapter | §4's `web_request_adapter`, as wasm gpu tests: `trunk`'s WebGPU backend when `requestAdapter` gives no adapter, and the WebGL fallback of `new_instance_with_webgpu_detection`. They test code that the series does not change; each test that replaces `requestAdapter` fails if the replacement is not installed. The wasm test build now compiles the WebGPU backend. | upstream tests these paths, or the tests are dropped |
| 38 | `8ae018bb7` | perf(core): stop asking backends once no later adapter can win | Review 4 item 4 (the owner's decision 2): with several backends, `request_adapter` asked every backend, so DX12 created a device for its selection, and GL its contexts, after an earlier backend had an adapter that none of theirs could rank before (DX12's device: 0.15 s for the RTX 5090 here, 2 s for the iGPU). It now stops asking there, for `request_adapter` only; the adapter returned and the error stay the same. Tested over 31,944 requests to three test backends: the test fails without the stop, and with a stop on any adapter or in `request_adapters` too. | a release contains it |
| 39 | `2c35267d7` | fix(core): ask no backend without the surface to select an adapter | Review 4 item 5 (the owner's decision 5): a request for a surface that has no surface for a backend still asked that backend to select, for a selection that could only be dropped. Such a backend is now enumerated without being asked, and wgpu-hal documents that `surface_hint` is `None` only for a request without a surface, and where an override may ignore it. Two noop hooks: the tests fail without the guard, and with a lazy step that passes no hint or does not check the selection against the surface. `selects_in_dxgi_order` tests DX12's surface path on a DirectComposition surface handle. | with 35's DX12 selection |
| 41 | (this commit) | docs: FORK.md: describe lazy adapter selection | This file | the branch is dropped |

Natural upstream PRs: 1, 2, 3, 6, 7, 8 and 24 each alone; the noop series 9 to 11; 12 and 13 after it (their tests use
it); the callbacks 14, 15 and 16; the surface series 17 to 23; 25 as two docs PRs (its row); 26 and 27 with the PRs
whose fixes they test. The queue-lock stage (third review item 12), after an upstream issue first:

- **PR A:** 28 alone (gfx-rs/wgpu#10675, which it needed, is in the base). Its tests and noop checks need commits 9 to 12 (third review
  item 12).
- **PR C:** 29 alone, with the window results.
- **PR B:** 30, a minimal lock that wgpu-hal exposes: no scopes, no injection, no public tracking, no wgpu-core hook.
  After C, whose test call log it moves and whose idle waits it locks.
- **PR D:** 31, internally synchronized queues, on B.
- **Present fences:** 33, after C and B (it keeps C's wait, under B's lock, as its fallback). Its tests give the device
  an instrumented lock through 32's injection; without 32 they would use B's handed-out lock instead.
- **Fork-only:** 32 stays here. Upstream gets it only if it asks for scopes or injection.

Lazy adapter selection: 35 with 36, 38 and 39 as #10011's next form, under a PR number of its own, since #10011 is
closed. 36 needs 8 and the noop series' commits 9 and 11 first, and is the PR's regression guard: on a CI runner whose
only DX12 adapter is WARP, the DX12 device test cannot fail without 35 (review 4 item 8). 35's core restructuring
(`find_adapters`, `suitable_adapters` and `rank_adapters`) with 38 needs neither the HAL hook nor DX12's selection, so
it can also be proposed apart from them; 38's tests need 8. 39 follows 36, whose noop hooks it extends, and needs the
noop series' 10 as well. 37 alone.

## The future of lazy adapter selection

The owner's decision 1 of review 4: keep 35 until gfx-rs/wgpu#10775 is released. That PR, open when review 4 read
it on 2026-11-14, adds a DX12 extension, on #10304's extension API, that restricts enumeration to the adapter of
one LUID. Then rebase onto it:

- drop 35's DX12 selection and `hal::Instance::request_adapter`, 36's lazy hooks, and 39, which guards the lazy
  step;
- OxQuick selects by LUID in `init_graphics`: `EnumAdapters1` order for `PowerPreference::None`,
  `EnumAdapterByGpuPreference` for `LowPower` and `HighPerformance`, software adapters only for
  `force_fallback_adapter`, and each LUID in turn until one gives an adapter (`oxquick-app` already depends on
  `windows`);
- keep 35 whole only if the released #10775 does not filter the enumeration that `request_adapter` uses.

Either way, 35's core restructuring and 38's early stop stay, as a core-only change without
`hal::Instance::request_adapter`: with `Backends::all()` and no LUID, a Vulkan adapter of the best rank still means
that DX12 and GL are never asked.

## Already upstream, not carried

- **The surface configuration lock and wait:** gfx-rs/wgpu#10296 (`91fe2f959`) holds `Surface::presentation`
  across `configure`, accepts submissions made during the wait, and destroys the acquired texture.
  gfx-rs/wgpu#10426 (`0ea906f07`) fixes the rank graph.
- **The front end's texture descriptors:** gfx-rs/wgpu#10100 (`95c30b295`) has `CoreTexture` hold the
  `Arc<Texture>` and read its descriptor. `VENDORED.md`'s `SurfaceOutput` and `dispatch` changes are not
  ported.
- **The queue exclusion:** gfx-rs/wgpu#10675 (`64bd5d5ab`, in the base) holds `Device::command_indices` around every HAL
  surface (un)configure and `Queue::drop`'s wait. That fixes the Vulkan `vkDeviceWaitIdle` race and the DX12
  idle fence shared by a device's surfaces. Commit 25 documents both; commit 26's `two_surfaces` tests them.
- **Lock ranks:** `trunk` already has the edges `VENDORED.md` added, plus #10675's. Commit 14 removes the
  `DEVICE_LOST_CLOSURE` rank (the closure's mutex is now unranked, like the error sink's). Commit 22 lets
  `SURFACE_PRESENTATION` be followed by the command allocator, the indirect validation buffer pool and a
  buffer's map state, which a failed preparing submission locks as it drops its command buffers and pending
  writes under the surface's lock.
- **Lock validation's lint and `std` (rows 4 and 5 before the rebase onto `60cef2be8`):**
  gfx-rs/wgpu#10964 (`722933f39`) makes 4's change, expecting the lock guards' saved field unused only
  with lock validation; gfx-rs/wgpu#10972 (`f62d5ed59`) fixes the build without `std` that 5 fixed, by
  taking the vanilla locks without `std` where 5 required it.
- **`PreviousOutputExists`:** dropped by the owner's rule. `trunk` destroys the acquired texture when
  configuring, as WebGPU does, and the branch keeps that. `trunk` still declares
  `ConfigureSurfaceError::PreviousOutputExists` but never returns it.

## Differences from `VENDORED.md` §2

- **Every resize recreates the swapchain** (first review, item 7). `trunk`'s `configure` always unconfigures
  the HAL surface before configuring it for the same device (from #10296; #10675 keeps it), so DX12's
  `ResizeBuffers` arm is unreachable and Vulkan never gets an `oldSwapchain`. The vendored copy reused the
  swapchain. Possible flicker, black frames or slower resizes; untested on hardware. **Check in the window
  slot** with a live resize of `arengine-demo` on DX12 and Vulkan.
- **A GPU wait that times out while configuring:** the vendored copy returned `GpuWaitTimeout` and logged it in
  drop. Commit 12 does the same, and keeps the surface's configuration, since the GPU may still use its
  textures. A texture still acquired is destroyed and given back to the HAL all the same, as when configuring
  succeeds. Unconfiguring and dropping log it and release the surface all the same.
- **Presenting a texture that the application destroyed** is a validation error (`TextureDestroyed`); its image
  is given back. The vendored copy silently swallowed `Lost`.
- **Callbacks:** the vendored copy deferred them site by site. Commits 14 and 16 queue all of them on the
  instance and deliver them at the end of the calls listed in `wgpu::Device::poll`'s documentation, and of
  every call that reports an error. A callback can therefore run inside any of those calls, on any thread, for
  any device of the instance: an application lock held across `present` or `submit` and taken by a callback
  deadlocks.

## Differences from `VENDORED.md` §1 and §4

- **One filter, not two** (commit 35). The PR, and so `VENDORED.md` §1, added `validate_requested_adapter`, a
  second copy of `request_adapter`'s per-backend filter, with a note to keep the two in sync. `trunk` has since
  moved that loop into `request_adapters` (`303def541`, 2026-10-26). Commit 35 makes the filter one function and
  the ranking another, used by `request_adapters` for every adapter and by `request_adapter` for each backend's
  selection first. A selection that does not suit the request records no reason, and the enumeration that
  follows records the backend's, as in the PR.
- **Which calls select lazily:** only `request_adapter`, as in the PR. `request_adapters` and
  `enumerate_adapters` expose every adapter.
- **Backends asked only until the result is certain** (commit 38, review 4's decision 2): the PR asked every
  backend, so DX12 created a device for its selection even when an earlier backend's adapter was certain to win.
  `request_adapter` now stops asking once it has an adapter that no later backend's adapter can rank before. 35
  keeps the PR's behaviour.
- **No selection for a surface that the backend has none for** (commit 39): the PR asked such a backend too, for a
  selection that could only be dropped.
- **Documentation:** wgpu-hal's `Instance::request_adapter` keeps `VENDORED.md`'s "should … may follow a native
  preference order", and states DXGI's preference as Microsoft documents it: integrated, discrete and external
  GPUs by class, with no word on how DXGI decides the class, and which a per-app GPU preference in Windows'
  graphics settings can supersede. wgpu-core's and wgpu's `request_adapter` now say that for `LowPower` and
  `HighPerformance` DX12's selection follows DXGI's order, so it can be another adapter of the same device type
  than a full enumeration ranks first, or, under a per-app GPU preference, an adapter of another type, and that
  with other backends a Vulkan adapter of the type the preference ranks first is still returned, so the per-app
  preference has no effect there (review 4's option A); that DX12 creates devices only for the adapters it tries
  up to its selection; and that the backends are asked in order only until no later one can outrank the adapter
  found. The PR said none of this.
- **The PR has not moved on** since the backport: upstream closed it at the three commits `VENDORED.md` backports
  (`d653d46a3`).
- **§4's tests** are where wgpu keeps such tests, not in a crate of their own:
  - `lazy_adapter` is the `lazy_adapter` module of `tests/tests/wgpu-validation/api/instance.rs`: noop tests in the
    CPU suite (commits 36 and 39), three of them with a surface, and `device_tests` on real adapters (commit 35,
    with 38's and 39's additions), which skip where there is no adapter of their kind. `VENDORED.md`'s open item,
    that its test passed on the pristine copy too, is answered: the DX12 test checks which adapters DX12 exposed,
    against DXGI and DX12's telemetry, and fails at the base, which exposes them all; the noop counters see the
    lazy step. The vendored test's rank check stays for `None`, which selects the first adapter enumerated, and
    for `force_fallback_adapter`, which selects a `Cpu` one. For a preference it is relaxed (option A): a device
    type other than that of the full enumeration's first adapter is printed, not failed, since a per-app GPU
    preference can choose it.
  - `web_request_adapter` is `tests/tests/wgpu-gpu/web_request_adapter.rs`, a `wasm_test` module of the wasm gpu
    tests. Its expectations are derived from `trunk`'s `webgpu.rs` and compare the whole `NotFound`; without
    `navigator.gpu` they also check the requested and supported backends.
  - `examples/startup_trace` is not carried: it is a measurement tool, not an example of wgpu's API, and
    `benches/` holds criterion benchmarks. The startup measurement used a probe outside the repository.
  - `surface_configure_race` stays dropped: `configure_race` covers it.

## Residual behaviour

- **No application callback runs under a wgpu-core lock**, by construction: only `PendingCallbacks::deliver`
  calls them, at the close of an outermost scope, and it asserts with lock validation that no lock is held.
  What remains outside that function:
  - the uncaptured error handler is called directly, inside the call that reported the error, outside the
    callback scopes so that its own calls deliver; every error report asserts that no lock is held;
  - `Device::drop` drops the application's closures and handler where the last reference to the device goes;
    wgpu-core's own references go with no lock held, which lock validation asserts;
  - callbacks still queued when the instance is dropped are dropped uncalled, which the drop asserts happens
    with no lock held. webgpu.h and Dawn call them with `CallbackCancelled` instead; wgpu has no such status for
    work-done callbacks. A device lost in a call that neither reports an error nor delivers callbacks
    (recording a pass, which reports at `end`) waits for the next call that delivers;
  - a callback that panics unwinds out of the call that delivered it, once that call's work is done, and a panic
    in a callback that `Queue::drop` delivers leaks the queue's allocation.
- **The assertions** above exist only with lock validation, which is on in wgpu's own debug builds and CI and
  off in applications that do not set the cfg.
- **A `SurfaceTexture` held across `configure`:** `trunk`'s behaviour, kept (first review, owner decision 2):
  configuring destroys its texture, and presenting it afterwards reports `NothingToPresent`, a validation error.
  Dropping it reports a validation error too, except while unwinding (commit 19), where it is logged.
- **A `SurfaceTexture` dropped while unwinding while another thread holds the snatch lock** (a submission, a
  poll, a mapping, encoding, an `as_hal` guard): the release cannot wait, so its texture stays usable, and its
  image acquired, until the next acquisition or configuration gives it back. A clone of the texture can be used
  meanwhile; configuring gives it back before it waits for the GPU. An `as_hal` guard held by the unwinding
  thread itself aborts the process under lock validation, a misuse the `as_hal` documentation forbids.
- **A write to a surface texture is a submission of its own** (commit 21): about 0.5 µs more per write on noop,
  one real submit on hardware. Batch such writes, or write to another texture and copy it.
- **Submitting from several threads on one device** costs about 9% more since commit 16 completed callback
  delivery. Measured on eddy2lnx (Ryzen 9950X, noop backend, release build, four threads submitting empty
  batches on one shared device, median of 7 processes): 1,175 ns per submit against 1,081 ns at commit 15 with
  the threads on one CCD, and 3,063 ns against 2,833 ns across two CCDs. Verifier 03's sketch of the atomic
  fast path alone measured the same, 1,174 ns against 1,073 ns. The cause is not established. The owner
  accepted this cost on 2026-11-11.
- **Vulkan's discarded image** (commit 7) is handed out again without a new `vkAcquireNextImageKHR`, so an
  `OUT_OF_DATE` after a resize shows up one frame later, and `wgpu` drops the `Outdated` that a present
  returns. `discard_texture` returns a `Result` that can no longer fail on Vulkan.
- **Resizing through the fallback teardown is slow on AMD's integrated GPUs** (the window gate, 2026-11-12/13).
  A device without `swapchain_maintenance1` waits for its queue to become idle, under the queue lock, at every
  swapchain rebuild (commit 29), and every resize rebuilds it (`trunk`'s `configure`). On Windows' AMD Radeon iGPU,
  which lists neither maintenance1 extension, the cube's live resize ran at about one frame per second, which
  matches wgpu-core's 1 s acquire timeout. On eddy2lnx's RADV under X11, commit 32's build (always the fallback)
  made 138 `xdotool` resizes in 30 s against 332 at the tip (present fences). Not bisected. The owner
  (2026-11-13): ledgered, to be fixed eventually; the AMD iGPU is not a target now.
- **GL bindings** (commit 3): the binding-to-slot table is now kept per pass kind and cloned into each program
  cache key, about 4 times the memory of the old table for huge binding numbers, which GL's
  `max_bindings_per_bind_group` of `u32::MAX` allows. Passthrough GLSL with hard-coded bindings shifts for
  layouts with bindings of the other pass kind or visible to no stage. The combined binding-point limits are
  still not queried.
- **Lazy adapter selection** (commits 35, 38 and 39):
  - For `LowPower` and `HighPerformance`, DX12 follows DXGI's preference order, so `request_adapter` need not
    return `request_adapters`' first adapter: it can return another adapter of the same device type, or, where a
    per-app GPU preference in Windows' graphics settings supersedes the requested one, an adapter of another type.
    This is option A, the owner's decision 3 of review 4: it follows Windows and Dawn, and OxQuick's
    `GpuPreference`, which maps to `DXGI_GPU_PREFERENCE_*`. With Vulkan in the instance, a Vulkan adapter of the
    preferred type still wins, so the Windows setting is lost there. `None` selects what a full enumeration ranks
    first.
  - A selection that does not suit the request (a surface it cannot present to, a limit bucket it does not fit,
    `adapter_allowed`'s checks) is dropped and the backend enumerated: that adapter's device is created twice, and
    so is each adapter's that DX12 tried before it. When no adapter DX12 tries exposes, each is tried twice.
    Without DXGI 1.6 (Windows 10 before 1803), DX12 declines `LowPower` and `HighPerformance`, which are
    enumerated as before; `None` still selects lazily in `EnumAdapters1` order.
  - With several backends, the backends are asked in order until one has an adapter that no later backend's can
    rank before (commit 38). So DX12 still creates a device for its selection when the earlier backends have no
    adapter of the best rank, even if one of theirs then wins a tie with it. OxQuick requests DX12 alone on
    Windows.
  - `wgpu_hal::Telemetry::d3d12_expose_adapter` now reports only the DX12 adapters that `request_adapter` tries.
    Firefox's telemetry counts them (per review 4, from summaries of its source): it would see fewer, and with
    `force_fallback_adapter` only the software adapters. None of the owner's consumers uses it.

## Proposal: texture-aware present, discard and release (first review, owner decision 2, not carried)

`wgpu-core`'s `present`, `discard` and `release` act on the surface's current texture, not on the caller's.
So a `SurfaceTexture` superseded by `configure` reports an error on present, and after a re-acquire, a stale
handle's discard or present acts on the **new** frame: it discards or presents the wrong one, without any
error. The proposal (first review, report 06, about a day, additive):

- each surface texture gets a state set under the surface's lock (`Acquired`, `Presented`, `Discarded`,
  `Superseded`; `configure` sets `Superseded`);
- new entry points that name the texture: `present_texture`, `discard_texture` and `release_texture`. Presenting
  a superseded texture returns `Ok(Status::Outdated)`, as a retired Vulkan swapchain does; presenting twice is
  a validation error; discarding and releasing anything but the acquired texture do nothing;
- `wgpu`'s `CoreSurfaceOutputDetail` keeps the texture and calls them; `present()`, `discard()` and
  `release()` keep their webgpu.h semantics for `deno_webgpu`, `player` and wgpu-native.

Raise it upstream as an issue citing gfx-rs/wgpu#10296, #9900 and #9360, then a PR. Carry it only if upstream
takes it, or if a product starts configuring from a thread other than the presenting one.

## Recorded, not ours

- **`deno_webgpu`** caches its surface texture and clears it only after a successful present. A configure,
  unconfigure or resize between `getCurrentTexture` and `present` leaves the context stuck: every later
  `getCurrentTexture` returns the destroyed texture, and every `present` fails. The spec's "expire the current
  texture" says configuring must clear it. A 3-line upstream fix; `deno_webgpu` is not ours.
- **`VENDORED.md` §5** still says the GL fix concerned textures that "the compute stage alone" reads. The
  vendored copy is read-only here; the owner corrects it.

## Raw submitters on wgpu's queue (commits 28 to 33)

Code that submits to wgpu's `VkQueue` itself, such as OxiVideo's FFmpeg and Vulkan Video lanes on a shared
device or an OpenXR runtime in `xrEndFrame`, serializes with wgpu through wgpu-hal's queue lock,
`wgpu_hal::vulkan::QueueLock`:

- wgpu-hal takes it innermost around exactly `vkQueueSubmit`, `vkQueuePresentKHR` and `vkQueueWaitIdle`, for
  the length of that call. Before each idle wait, wgpu-core waits on the fence for wgpu's own submissions with
  no lock held, then checks under `command_indices` that none came since (commit 28). So the lock is never held
  across a wait for wgpu's work, which could wait on a semaphore that the other code signals.
- The application gives its own lock in `DeviceQueue::lock` to `Adapter::device_from_raw` (commit 32), which
  wgpu-hal then always takes, or takes wgpu-hal's through `as_hal` (`Queue::lock_queue`, `Queue::queue_lock`,
  also on `Device`; commit 30). The lock is a `lock(scope)`/`unlock(scope)` pair, scoped to one queue or every
  queue, so that FFmpeg's `lock_queue`/`unlock_queue` callbacks can implement or call it. Several wgpu-hal
  devices made from one `VkDevice` must be given one lock. A second wgpu-core device on the same raw queue is
  foreign work to the first: give both the same lock.
- A queue created with `VK_DEVICE_QUEUE_CREATE_INTERNALLY_SYNCHRONIZED_BIT_KHR` takes no lock unless given one
  (commit 31).
- Swapchain teardown waits for its presents only: on their fences, without the lock, where the device has
  `swapchainMaintenance1` (commit 33), else for wgpu's queue to become idle, under the lock (commit 29). It
  never waits for the device, so the device's other queues need no lock against wgpu.
- A holder of the lock must not call into wgpu, wgpu-hal, or stage a semaphore, nor may a logger or debug
  messenger that runs inside a Vulkan call take it. Debug builds check it for locks taken through wgpu-hal; the
  wgpu-core half of the check needs lock validation (`--cfg wgpu_validate_locks_debug`). `vkQueuePresentKHR`
  can block in the driver while wgpu-hal holds the lock.
- Not covered: `vkDestroyDevice` requires every queue of the device to be unused, which no lock can provide.
  Without present fences, a foreign batch on the shared queue that waits on a semaphore that only a later
  submission to the same queue signals keeps the teardown's idle wait, which holds the lock, from ending.
- From libavutil 62 on, FFmpeg has no lock callbacks: a queue it shares with wgpu must then be internally
  synchronized (on every queue of the device that FFmpeg uses), or FFmpeg must have a queue of its own.
- **One lock per topology** (the owner's decision 2 of the third fix round): wgpu-interop stops staging
  semaphores into wgpu's batches and submits raw wait-only and signal-only batches around `wgpu::Queue::submit`
  under the queue lock (third review item 9). wgpu's semaphore staging is unchanged. The consumers' changes are
  written out in the lane's report, not applied.

DX12's `ID3D12CommandQueue` and Metal's `MTLCommandQueue` need no lock: both APIs serialize submissions
themselves. GLES already exposes its context lock (`AdapterContext::lock`).

Upstream: no issue, PR or accepted design adds a queue lock (searched 2026-11-11) or present fences
(searched 2026-11-12). #10675's "Not in this PR" names the `vkDeviceWaitIdle` gap, without a maintainer reply. The
precedents are ANGLE's `eglLockVulkanQueueANGLE`, libplacebo's `lock_queue`, vkd3d's `vkd3d_acquire_vk_queue` and
DXVK's `LockSubmissionQueue`; for present fences, libplacebo and ANGLE.

## Upstream review points

- **Policy:** `AGENTS.md` forbids fully agentic PRs and agent co-authorship, and `CONTRIBUTING.md` asks
  contributors to write descriptions and large comment blocks themselves. The commits carry no
  `Co-Authored-By` trailers (removed on 2026-11-11, the owner's decision). Their messages and larger comment
  blocks are the owner's to rewrite before any PR.
- **GL texture units (commit 3):** per pass kind, not per pipeline as Dawn does. Dawn applies bind groups at
  each draw for the current pipeline; wgpu-core sets a bind group again only when its layout changes, so
  per-pipeline units would mean deferring all binding and rebinding on every pipeline switch. Per pass kind
  keeps binding in `set_bind_group` and fits the 32 units GLES guarantees.
- **The noop device type (commit 8):** `None` reports `Cpu`, as the code always did; the documentation said
  `Other`, which would rank an enabled noop adapter above software adapters under a power preference and
  exclude it from `force_fallback_adapter`. An owner decision; upstream may prefer the documented default.
- **Callback delivery (commits 14 and 16):** an implicit `ProcessEvents` at the end of each delivering call,
  per instance, as webgpu.h's instance-level `ProcessEvents` and Dawn's `EventManager` are. wgpu-core's
  `Device::get_queue` is no longer public.
- **The noop backend** guards its state with a spin lock: without `std`, `wgpu_sync::Mutex` is not `Sync`.
  Its module keeps `#![allow(unused_variables)]` from `trunk`.
- **lock_api 0.5** (gfx-rs/wgpu#10894 widened wgpu to `lock_api >=0.4.6, <0.6.0`): commit 30's own lock keeps no
  guard state, and its file compiles and passes its unit tests against lock_api 0.5.0 and parking_lot 0.13.0 in a
  scratch crate. The branch's `Cargo.lock` stays on lock_api 0.4.14.
- **gfx-rs/wgpu#10304** reworks the `open_with_callback`/`CreateDeviceCallback` code that commit 31 edits;
  it is not in the base. gfx-rs/wgpu#10895 and #8388, upstream since 2026-11-11, are: 14 gives wgpu-core's
  `Device::queue` for #10895's `wgpu::Device::queue`, and 29 and 30 make #8388's DXGI-swapchain teardown wait
  for the device's queue under the queue lock.
- **Validation layers older than Vulkan 1.4.321** do not know `VK_KHR_swapchain_maintenance1` (promoted in
  1.4.321). They report `VUID-VkDeviceCreateInfo-pNext-pNext` at every device creation that enables it with its
  feature struct, whose structure type the KHR and EXT extensions share: they name the EXT extension as its only
  parent. Seen with SDK 1.4.304 (header 304) on the RTX 5090; the 1.4.341 layer on eddy2lnx accepts it. wgpu's tests
  fail on any validation message, so they need a layer of 1.4.321 or later wherever a driver lists the KHR extension.
- **CI's wasm32 row `clippy --tests --features glsl,spirv` failed on commit 16 before this rebase:** commit 16's
  test `concurrent_deliveries_call_every_callback_once` shares an instance's `PendingCallbacks` between threads,
  and `PendingCallbacks` is `Sync` only with `send_sync`, without which its queued callbacks need not be `Send`:
  four E0277 errors, one per kind of callback. 16 now builds the test only with `send_sync`: the rebase folded
  commit 40, which did, into it (the owner's decision 4 of review 4). The step's other five commands, three clippy
  and two `doc`, passed before it too.
- **The CHANGELOG** entries of commits 28 to 33 are cut to `trunk`'s length; 28 and 31 are `BREAKING:`, and 28
  is `fix(core)!:` (it tightens `unsafe` contracts).

## Tests not run here

- **The window gate** (the owner's decision 3 of the third fix round) ran on 2026-11-12/13 with the owner:
  `tests/tests/wgpu_surface_operations.rs` (commits 26 and 27), `trunk`'s `tests/tests/wgpu_surface.rs`, a live
  resize of `wgpu-examples cube` on Vulkan, `configure_race` alone per adapter at the tip and at commit 27
  (`8a42f5dab`'s library), and commit 32's build (no present fences). The kit and its logs are in the lane's
  report. **The branch adds no failure on any platform.**
  - **Windows** (RTX 5090, AMD Radeon iGPU, WARP; DX12, Vulkan, GL): the five trials on every adapter,
    `wgpu_surface` at the tip, the base and commit 32, and `configure_race` alone on all six adapters at the tip
    and the base pass. The cube's live resize by hand: on the 5090 at the tip (present fences) and at commit 32,
    nothing unusual; on the AMD iGPU at the tip (the fallback), about one frame per second (Residual behaviour).
  - **eddy2lnx, Wayland** (GNOME, the display on NVIDIA): NVIDIA Vulkan passes everything (the five trials,
    `wgpu_surface`, `configure_race` at the tip and the base, commit 32's trials, four cube drags), its teardown
    on present fences. AMD fails at the base too: mutter cannot import RADV's dmabufs on the NVIDIA-driven
    display, and RADV reports no present support there. NVIDIA GL does not support the surface (environment).
  - **eddy2lnx, X11** (bare Xorg on display `:5`, VT4, openbox, no compositor; unattended, `xdotool` resizes):
    on NVIDIA and AMD (PRIME), the five trials and `configure_race` pass at the tip and the base, the cube
    survives every resize stream, and commit 32's trials pass; both teardowns on present fences.
  - **`trunk`'s `wgpu_surface` fails under X11 at the base as at the tip,** on both GPUs (NVIDIA: `Outdated`; AMD:
    `Suboptimal`). The test configures 256×256, then 512×512, on an invisible window that keeps winit's 800×600,
    and takes only `Success` from the first acquire; Mesa's X11 WSI marks such a swapchain `SUBOPTIMAL` at creation
    by design. This is `trunk`'s test, not the branch: noted here only (the owner, 2026-11-13).
- Commit 7 (Vulkan discard) is tested only by commit 26's `discarded_images`.
- Commit 24 (Metal) and commit 21's `copy_external_image_to_texture` (WebGL) were type-checked with Rust 1.99.0
  for `aarch64-apple-ios` and `wasm32-unknown-unknown`; nothing ran on those platforms.
- The Vulkan device tests (wgpu-hal's `vulkan::device_tests`, the `wgpu_vulkan` binary) open devices and skip at
  run time where there is no Vulkan; they run on CPU adapters too. The lane's CPU suites leave them out by name
  (`device_tests::`); the lane runs them under the GPU lock. wgpu-hal's need `VK_EXT_headless_surface` (Mesa has it,
  NVIDIA's and the Windows drivers do not), so headless, the present-fence path ran on RADV only; NVIDIA's ran in
  the window gate on eddy2lnx (Wayland and X11, above). The internally synchronized queue's tests need
  `VK_KHR_internally_synchronized_queues` (NVIDIA 615 on eddy2lnx has it, RADV there does not).
- The lazy adapter's device tests (commits 35, 38 and 39) expose the adapters of every native backend: only DX12's
  exposure creates a device, GL creates contexts, and Vulkan and Metal create none. They skip where there is no
  adapter of their kind, and `dx12::selects_in_dxgi_order` skips without DXGI 1.6. They ran under the GPU lock here
  (RTX 5090, AMD Radeon iGPU, WARP; DX12, Vulkan and GL), last in review 4's fix round, with the early stop and the
  DirectComposition surface variant: DX12 exposes DXGI's adapters only up to its selection, alone and for a
  surface, and none where a Vulkan adapter ranks first; the base fails the DX12 test. Before that round they ran
  on eddy2lnx too (Vulkan and GL on each GPU). Their skips were compiled, not run: every machine here has the
  adapters.
- wgpu-hal's DX12 code was checked with its MSRV, 1.87, for `x86_64-pc-windows-msvc` on eddy2lnx, as CI's Windows
  MSRV rows do, at each of commits 35 to 41. That compiles the libraries, not the tests.
- Option A's case, a per-app GPU preference that gives DX12's selection another device type, was not reproduced:
  it needs per-app graphics settings in the owner's registry, which the lane does not write.
- The wasm gpu tests, `web_request_adapter` among them (commit 37), ran in the fork's rig here, last in review 4's
  fix round (432/432): headless Chromium 149,
  whose `navigator.gpu` exists and whose `requestAdapter` resolves `null`, and WebGL on SwiftShader. The three tests
  that replace `requestAdapter` check that wgpu called the replacement, and fail when it is not installed.
