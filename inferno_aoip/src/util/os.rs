use thread_priority::Error;

#[cfg(all(target_family = "unix", not(target_os = "macos")))]
pub fn set_current_thread_realtime(priority_hint: u8) -> Result<(), Error> {
  use thread_priority::unix::set_thread_priority_and_policy;
  use thread_priority::{
    thread_native_id, RealtimeThreadSchedulePolicy, ThreadPriority, ThreadPriorityValue,
    ThreadSchedulePolicy,
  };

  set_thread_priority_and_policy(
    thread_native_id(),
    ThreadPriority::Crossplatform(ThreadPriorityValue::try_from(priority_hint).unwrap()),
    ThreadSchedulePolicy::Realtime(RealtimeThreadSchedulePolicy::Fifo),
  )
}

#[cfg(target_os = "macos")]
pub fn set_current_thread_realtime(priority_hint: u8) -> Result<(), Error> {
  set_thread_time_constraint(unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) }, priority_hint)
}

/// Puts the given mach thread into THREAD_TIME_CONSTRAINT_POLICY (the macOS equivalent of
/// SCHED_FIFO used by Core Audio I/O threads). The priority hint has no meaning for this policy.
#[cfg(target_os = "macos")]
fn set_thread_time_constraint(thread: libc::mach_port_t, _priority_hint: u8) -> Result<(), Error> {
  use std::mem::size_of;

  const PERIOD_NS: u64 = 1_000_000;
  const COMPUTATION_NS: u64 = 200_000;
  const CONSTRAINT_NS: u64 = 500_000;

  // mach_timebase_info is deprecated in libc in favour of the mach2 crate, but still works
  #[allow(deprecated)]
  let (numer, denom) = {
    let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
    let kr = unsafe { libc::mach_timebase_info(&mut timebase) };
    if kr != libc::KERN_SUCCESS || timebase.numer == 0 || timebase.denom == 0 {
      return Err(Error::OS(kr as i32));
    }
    (timebase.numer as u64, timebase.denom as u64)
  };
  let ns_to_ticks = |ns: u64| (ns * denom / numer) as u32;

  let mut policy = libc::thread_time_constraint_policy {
    period: ns_to_ticks(PERIOD_NS),
    computation: ns_to_ticks(COMPUTATION_NS),
    constraint: ns_to_ticks(CONSTRAINT_NS),
    preemptible: 1,
  };
  let count = (size_of::<libc::thread_time_constraint_policy>() / size_of::<libc::integer_t>())
    as libc::mach_msg_type_number_t;
  let kr = unsafe {
    libc::thread_policy_set(
      thread,
      libc::THREAD_TIME_CONSTRAINT_POLICY as _,
      &mut policy as *mut libc::thread_time_constraint_policy as libc::thread_policy_t,
      count,
    )
  };
  if kr != libc::KERN_SUCCESS {
    return Err(Error::OS(kr as i32));
  }
  Ok(())
}

#[cfg(not(target_family = "unix"))]
pub fn set_current_thread_realtime(_priority_hint: u32) -> Result<(), Error> {
  thread_priority::set_current_thread_priority(ThreadPriority::Max)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
  use super::*;

  #[test]
  fn realtime_succeeds_in_normal_process() {
    // in a normal process, thread_policy_set(THREAD_TIME_CONSTRAINT_POLICY) succeeds (verified on real hardware)
    std::thread::spawn(|| set_current_thread_realtime(80)).join().unwrap().unwrap();
  }

  #[test]
  fn realtime_failure_is_err_not_panic() {
    // calling with an invalid mach port (0) must go through the Err path
    assert!(set_thread_time_constraint(0, 80).is_err());
  }
}
