//! Whether this machine can run WebGPU, which on Linux means whether Vulkan reaches a real GPU.
//!
//! WebGPU implementations (Dawn, wgpu) sit on the platform's native graphics API: Metal on macOS, Direct3D 12 on
//! Windows and Vulkan on Linux. The first two ship with every supported version of their operating system, so the
//! answer there is always yes. Linux ships no graphics API at all: Vulkan needs the loader (`libvulkan.so.1`, packaged
//! as `libvulkan1`, `vulkan-loader` or `vulkan-icd-loader` depending on the distribution) *and* a driver for the GPU —
//! Mesa's `mesa-vulkan-drivers` for AMD, Intel and NVK, or the one the proprietary NVIDIA driver installs itself.
//!
//! Package names are not asked about, because they differ between distributions and say nothing about the driver
//! actually loading. Vulkan itself is asked instead: an instance is created and its physical devices listed, which is
//! exactly what a WebGPU implementation does first. A device only counts if it is a GPU — Mesa's `lavapipe`, installed
//! with `mesa-vulkan-drivers`, is a CPU rasteriser that reports itself as a Vulkan device, and WebGPU on it is slower
//! than running on the CPU directly — and supports Vulkan 1.1, the oldest version Dawn accepts.
//!
//! Vulkan is loaded at runtime rather than linked, because a machine without the loader must still be able to load
//! this crate. Only the probe is Linux-specific; the rule deciding what counts is compiled everywhere so it stays
//! testable from any host.
//!
//! Nothing the probe loads is ever unloaded. Creating an instance loads every installed driver, and drivers are not
//! written to be unloaded: they register thread-local destructors and exit handlers that assume their code stays
//! mapped for the life of the process. Under WSL2, Mesa's `dzn` driver loads `libd3d12core.so`, whose destructor then
//! runs on a thread that exits long after the probe has returned — into code that is no longer there, which is a
//! segfault in whatever the process happens to be doing by then. So every library the probe caused to be loaded,
//! the Vulkan loader included, is marked `RTLD_NODELETE` before the instance is destroyed.

/// `VK_API_VERSION_1_1`, the oldest Vulkan version Dawn accepts a device at.
pub(super) const VULKAN_1_1: u32 = (1 << 22) | (1 << 12);

/// `VK_PHYSICAL_DEVICE_TYPE_CPU`, the type `lavapipe` and other software rasterisers report.
pub(super) const DEVICE_TYPE_CPU: i32 = 4;

/// Says whether any of `devices`, each an `(apiVersion, deviceType)` pair from `VkPhysicalDeviceProperties`, can run
/// WebGPU: a device that is not a CPU and supports Vulkan 1.1 or newer.
pub(super) fn supports_webgpu(devices: impl IntoIterator<Item = (u32, i32)>) -> bool {
    // The top three bits are the API *variant*, which is 0 for Vulkan and would otherwise outweigh the version.
    devices
        .into_iter()
        .any(|(api_version, device_type)| device_type != DEVICE_TYPE_CPU && api_version & 0x1FFF_FFFF >= VULKAN_1_1)
}

/// Says whether this machine can run WebGPU.
///
/// On macOS and Windows this is always `true`: WebGPU runs on Metal and Direct3D 12, which every supported version of
/// those systems ships. On Linux it runs on Vulkan, so this loads the Vulkan loader, creates an instance and returns
/// `true` only if it lists a GPU — not a CPU rasteriser such as `lavapipe` — supporting Vulkan 1.1 or newer. A machine
/// missing `libvulkan1` or a Vulkan driver (`mesa-vulkan-drivers`, or the proprietary NVIDIA driver's own) answers
/// `false`. On any other operating system the answer is `false`.
///
/// Nothing is cached, and on Linux each call creates and destroys a Vulkan instance, which loads every installed
/// driver. The drivers stay loaded for the rest of the process, because unloading one is not safe. A caller asking
/// more than once should hold onto the answer.
///
/// A `true` is what the machine offers, not a promise that every WebGPU implementation will accept it: an adapter can
/// still lack a feature or limit one of them requires.
///
/// ```
/// use rust_sak::sysinfo::is_webgpu_supported;
///
/// if is_webgpu_supported() {
///     println!("WebGPU can run here");
/// }
/// ```
#[must_use]
pub fn is_webgpu_supported() -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        true
    }

    #[cfg(target_os = "linux")]
    {
        imp::probe()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::HashSet;
    use std::ffi::{CStr, CString, c_char, c_int, c_void};

    use libloading::{Library, Symbol};

    use super::supports_webgpu;

    /// The loader's soname, which every distribution's package installs.
    const LIBRARY: &str = "libvulkan.so.1";

    /// `VK_SUCCESS`.
    const SUCCESS: i32 = 0;

    /// `VK_INCOMPLETE`: the device list was truncated, which still leaves every device written usable.
    const INCOMPLETE: i32 = 5;

    /// `VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO`.
    const INSTANCE_CREATE_INFO: i32 = 1;

    /// An opaque, dispatchable `VkInstance`.
    type Instance = *mut c_void;

    /// An opaque, dispatchable `VkPhysicalDevice`.
    type PhysicalDevice = *mut c_void;

    /// `PFN_vkVoidFunction`, what `vkGetInstanceProcAddr` returns before it is cast to the real signature.
    type VoidFunction = Option<unsafe extern "system" fn()>;

    type GetInstanceProcAddr = unsafe extern "system" fn(Instance, *const c_char) -> VoidFunction;
    type CreateInstance = unsafe extern "system" fn(*const InstanceCreateInfo, *const c_void, *mut Instance) -> i32;
    type DestroyInstance = unsafe extern "system" fn(Instance, *const c_void);
    type EnumeratePhysicalDevices = unsafe extern "system" fn(Instance, *mut u32, *mut PhysicalDevice) -> i32;
    type GetPhysicalDeviceProperties = unsafe extern "system" fn(PhysicalDevice, *mut c_void);

    /// `VkInstanceCreateInfo`. No application info is passed, which asks for Vulkan 1.0 — the one version every loader
    /// accepts. The version a *device* supports is reported regardless of the instance's.
    #[repr(C)]
    struct InstanceCreateInfo {
        s_type: i32,
        p_next: *const c_void,
        flags: u32,
        p_application_info: *const c_void,
        enabled_layer_count: u32,
        pp_enabled_layer_names: *const *const c_char,
        enabled_extension_count: u32,
        pp_enabled_extension_names: *const *const c_char,
    }

    /// The leading fields of `VkPhysicalDeviceProperties`, the only ones read.
    ///
    /// The whole structure is 824 bytes of names, limits and sparse properties; declaring all of it to read two
    /// integers would be most of this file. The driver writes into [`PropertiesBuffer`] instead, which is larger than
    /// the full structure, and this header is read from its start.
    #[repr(C)]
    struct PropertiesHeader {
        api_version: u32,
        _driver_version: u32,
        _vendor_id: u32,
        _device_id: u32,
        device_type: i32,
    }

    /// Room for a whole `VkPhysicalDeviceProperties` with margin, aligned for its 8-byte fields.
    type PropertiesBuffer = [u64; 128];

    /// Loads Vulkan and says whether it lists a device that can run WebGPU.
    ///
    /// Every failure — no loader, no driver, an instance the loader refuses — is a `false`: a machine without Vulkan is
    /// exactly the case this exists to report.
    pub(in super::super) fn probe() -> bool {
        // Taken before the loader is opened, so the loader itself is among what gets pinned: dropping `library` below
        // would otherwise unload it, and every driver it holds with it.
        let before = loaded_objects();

        // SAFETY: loading the Vulkan loader runs its initialisers, which it ships precisely for a process to load it.
        let Ok(library) = (unsafe { Library::new(LIBRARY) }) else {
            return false;
        };

        // SAFETY: each entry point is cast to the signature `vulkan_core.h` declares for it, and none outlives
        // `library`.
        unsafe { devices(&library, &before) }.is_some_and(supports_webgpu)
    }

    /// Creates an instance, reads every physical device's version and type, and destroys it again — after pinning
    /// every library loaded since `before`, since destroying the instance is what unloads the drivers.
    ///
    /// # Safety
    ///
    /// `library` must be the Vulkan loader.
    unsafe fn devices(library: &Library, before: &HashSet<CString>) -> Option<Vec<(u32, i32)>> {
        // SAFETY: `vkGetInstanceProcAddr` is the loader's one guaranteed export, with this signature.
        let get_proc: Symbol<GetInstanceProcAddr> = unsafe { library.get(b"vkGetInstanceProcAddr\0") }.ok()?;

        // SAFETY: a null instance is how global commands such as `vkCreateInstance` are looked up.
        let create: CreateInstance =
            unsafe { std::mem::transmute(get_proc(std::ptr::null_mut(), c"vkCreateInstance".as_ptr())?) };

        let info = InstanceCreateInfo {
            s_type: INSTANCE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            p_application_info: std::ptr::null(),
            enabled_layer_count: 0,
            pp_enabled_layer_names: std::ptr::null(),
            enabled_extension_count: 0,
            pp_enabled_extension_names: std::ptr::null(),
        };

        let mut instance: Instance = std::ptr::null_mut();
        // SAFETY: `info` is a valid create-info that outlives the call, there is no allocator, and `instance` is a
        // valid out-pointer.
        if unsafe { create(&info, std::ptr::null(), &mut instance) } != SUCCESS || instance.is_null() {
            // A driver can have run its initialisers before the instance was refused, so it is pinned all the same.
            pin_loaded_since(before);
            return None;
        }

        // SAFETY: `instance` was just created, and is destroyed below before `library` can be dropped.
        let devices = unsafe { read_devices(&get_proc, instance) };

        // After the devices are read, because a driver can load more of itself while listing them — `dzn` only loads
        // `libd3d12.so` then — and before the instance is destroyed, because that is when the loader closes them.
        pin_loaded_since(before);

        // SAFETY: looked up for the same instance, which nothing uses afterwards.
        if let Some(destroy) = unsafe { get_proc(instance, c"vkDestroyInstance".as_ptr()) } {
            // SAFETY: `vkDestroyInstance` has this signature, and `instance` was created without an allocator.
            unsafe {
                std::mem::transmute::<unsafe extern "system" fn(), DestroyInstance>(destroy)(instance, std::ptr::null())
            };
        }

        devices
    }

    /// Lists `instance`'s physical devices as `(apiVersion, deviceType)` pairs.
    ///
    /// # Safety
    ///
    /// `instance` must be a live instance created through `get_proc`'s loader.
    unsafe fn read_devices(get_proc: &Symbol<GetInstanceProcAddr>, instance: Instance) -> Option<Vec<(u32, i32)>> {
        // SAFETY: each name is looked up for a live instance, and cast to the signature `vulkan_core.h` declares.
        let (enumerate, properties) = unsafe {
            let enumerate: EnumeratePhysicalDevices =
                std::mem::transmute(get_proc(instance, c"vkEnumeratePhysicalDevices".as_ptr())?);
            let properties: GetPhysicalDeviceProperties =
                std::mem::transmute(get_proc(instance, c"vkGetPhysicalDeviceProperties".as_ptr())?);

            (enumerate, properties)
        };

        let mut count: u32 = 0;
        // SAFETY: a null array asks only for the count, written to a valid out-pointer.
        if unsafe { enumerate(instance, &mut count, std::ptr::null_mut()) } != SUCCESS {
            return None;
        }

        let mut handles: Vec<PhysicalDevice> = vec![std::ptr::null_mut(); count as usize];
        // SAFETY: `handles` holds the `count` entries the call is told it may write.
        let status = unsafe { enumerate(instance, &mut count, handles.as_mut_ptr()) };
        if status != SUCCESS && status != INCOMPLETE {
            return None;
        }
        handles.truncate(count as usize);

        let devices = handles
            .into_iter()
            .map(|device| {
                let mut buffer: PropertiesBuffer = [0; 128];
                // SAFETY: `device` came from this instance, and `buffer` is larger than the structure written to it.
                unsafe { properties(device, buffer.as_mut_ptr().cast()) };

                // SAFETY: the buffer starts with a `VkPhysicalDeviceProperties`, whose leading fields are the header,
                // and its 8-byte alignment exceeds the header's.
                let header = unsafe { &*buffer.as_ptr().cast::<PropertiesHeader>() };
                (header.api_version, header.device_type)
            })
            .collect();

        Some(devices)
    }

    /// The path of every shared object in the process, as the dynamic loader recorded it.
    ///
    /// The main executable and the vDSO are reported with an empty name and are left out: neither can be unloaded.
    fn loaded_objects() -> HashSet<CString> {
        unsafe extern "C" fn collect(info: *mut libc::dl_phdr_info, _size: libc::size_t, data: *mut c_void) -> c_int {
            // SAFETY: `data` is the `HashSet` passed below, and `info` is valid for the duration of the callback.
            let (objects, name) = unsafe { (&mut *data.cast::<HashSet<CString>>(), (*info).dlpi_name) };

            if !name.is_null() {
                // SAFETY: a non-null `dlpi_name` is a NUL-terminated string owned by the loader.
                let name = unsafe { CStr::from_ptr(name) };
                if !name.is_empty() {
                    objects.insert(name.to_owned());
                }
            }

            0
        }

        let mut objects = HashSet::new();
        // SAFETY: `collect` only reads `info` and writes into `objects`, which outlives the call.
        unsafe { libc::dl_iterate_phdr(Some(collect), (&raw mut objects).cast()) };

        objects
    }

    /// Marks every shared object loaded since `before` as never to be unloaded.
    ///
    /// Collected first and pinned afterwards, rather than from inside the `dl_iterate_phdr` callback, which runs with
    /// the loader's lock held. An object that cannot be pinned is skipped: this is a safety net for the probe, and a
    /// failure to pin one is no reason to report that Vulkan is missing.
    fn pin_loaded_since(before: &HashSet<CString>) {
        for name in loaded_objects().difference(before) {
            // The handle is left open: the object can no longer be unloaded, so a close would release nothing.
            //
            // SAFETY: `RTLD_NOLOAD` only looks up an object that is already loaded, so no initialiser runs; the flag
            // `RTLD_NODELETE` is what keeps its code mapped for the rest of the process.
            unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD | libc::RTLD_NODELETE) };
        }
    }
}
