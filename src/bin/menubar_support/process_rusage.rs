// Wraps the public libproc `proc_pid_rusage`/`proc_pidinfo` APIs to get the
// same memory breakdown Activity Monitor shows when you double-click a
// process (Real Memory / Physical Footprint / Virtual). sysinfo only
// exposes RSS and virtual size, not the phys_footprint figure Activity
// Monitor's process list "Memory" column is actually based on — these are
// the officially documented (non-private) ways to read all three, declared
// in <libproc.h>/<sys/resource.h>/<sys/proc_info.h>.
//
// Shared/Private memory (the two extra fields Activity Monitor's
// double-click inspector shows) aren't included here: getting those
// reliably needs `task_info()` over a Mach task port, which macOS denies
// for other users'/other apps' processes without root or a special
// entitlement — proc_pidinfo/proc_pid_rusage work for any process the
// current user owns without any of that.

#[repr(C)]
#[derive(Default)]
struct RusageInfoV4 {
    ri_uuid: [u8; 16],
    ri_user_time: u64,
    ri_system_time: u64,
    ri_pkg_idle_wkups: u64,
    ri_interrupt_wkups: u64,
    ri_pageins: u64,
    ri_wired_size: u64,
    ri_resident_size: u64,
    ri_phys_footprint: u64,
    ri_proc_start_abstime: u64,
    ri_proc_exit_abstime: u64,
    ri_child_user_time: u64,
    ri_child_system_time: u64,
    ri_child_pkg_idle_wkups: u64,
    ri_child_interrupt_wkups: u64,
    ri_child_pageins: u64,
    ri_child_elapsed_abstime: u64,
    ri_diskio_bytesread: u64,
    ri_diskio_byteswritten: u64,
    ri_cpu_time_qos_default: u64,
    ri_cpu_time_qos_maintenance: u64,
    ri_cpu_time_qos_background: u64,
    ri_cpu_time_qos_utility: u64,
    ri_cpu_time_qos_legacy: u64,
    ri_cpu_time_qos_user_initiated: u64,
    ri_cpu_time_qos_user_interactive: u64,
    ri_billed_system_time: u64,
    ri_serviced_system_time: u64,
    ri_logical_writes: u64,
    ri_lifetime_max_phys_footprint: u64,
    ri_instructions: u64,
    ri_cycles: u64,
    ri_billed_energy: u64,
    ri_serviced_energy: u64,
    ri_interval_max_phys_footprint: u64,
    ri_runnable_time: u64,
}

const RUSAGE_INFO_V4: i32 = 4;

// Layout matches `struct proc_taskinfo` in <sys/proc_info.h>; only the
// first two fields (virtual/resident size) are used here, but the struct
// must be the kernel's full expected size for `proc_pidinfo` to fill it in
// correctly.
#[repr(C)]
#[derive(Default)]
struct ProcTaskInfo {
    pti_virtual_size: u64,
    pti_resident_size: u64,
    pti_total_user: u64,
    pti_total_system: u64,
    pti_threads_user: u64,
    pti_threads_system: u64,
    pti_policy: i32,
    pti_faults: i32,
    pti_pageins: i32,
    pti_cow_faults: i32,
    pti_messages_sent: i32,
    pti_messages_received: i32,
    pti_syscalls_mach: i32,
    pti_syscalls_unix: i32,
    pti_csw: i32,
    pti_threadnum: i32,
    pti_numrunning: i32,
    pti_priority: i32,
}

const PROC_PIDTASKINFO: i32 = 4;

// Part of libSystem, already linked into every macOS binary — no extra
// #[link] needed.
unsafe extern "C" {
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV4) -> i32;
    fn proc_pidinfo(
        pid: i32,
        flavor: i32,
        arg: u64,
        buffer: *mut ProcTaskInfo,
        buffersize: i32,
    ) -> i32;
}

pub struct ProcessRusage {
    pub resident_size: u64,
    pub phys_footprint: u64,
    pub virtual_size: u64,
}

pub fn rusage_for_pid(pid: i32) -> Option<ProcessRusage> {
    let mut info = RusageInfoV4::default();
    // SAFETY: `info` is a plain-old-data struct matching the kernel's
    // rusage_info_v4 layout, sized and zeroed before the call; the kernel
    // only ever writes within its bounds for this flavor.
    let result = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V4, &mut info) };
    if result != 0 {
        return None;
    }

    let mut task_info = ProcTaskInfo::default();
    let size = std::mem::size_of::<ProcTaskInfo>() as i32;
    // SAFETY: same reasoning as above, matching struct proc_taskinfo's
    // documented layout and size.
    let task_result = unsafe { proc_pidinfo(pid, PROC_PIDTASKINFO, 0, &mut task_info, size) };
    let virtual_size = if task_result == size {
        task_info.pti_virtual_size
    } else {
        0
    };

    Some(ProcessRusage {
        resident_size: info.ri_resident_size,
        phys_footprint: info.ri_phys_footprint,
        virtual_size,
    })
}
