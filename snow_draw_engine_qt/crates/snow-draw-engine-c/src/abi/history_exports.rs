use crate::abi::handles::*;
use crate::abi::types::*;

/// # Safety
/// `runtime` must be a live runtime; `out_state` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_runtime_host_history_state(
    runtime: SnowRuntime,
    direction: i32,
    out_state: *mut u64,
) -> SnowError {
    ffi_error(|| {
        if out_state.is_null() || !(-1..=1).contains(&direction) {
            return SnowError::InvalidArgument;
        }
        ffi_status(with_runtime_ref(runtime, |runtime| {
            write_out(out_state, runtime.host_history_state(direction));
            Ok(())
        }))
    })
}

/// # Safety
/// `runtime` must be live; `out_changed_viewports` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_runtime_commit_host_history_state(
    runtime: SnowRuntime,
    state: u64,
    out_changed_viewports: *mut SnowChangedViewportList,
) -> SnowError {
    ffi_error(|| {
        if out_changed_viewports.is_null() {
            return SnowError::InvalidArgument;
        }
        ffi_status(with_runtime_impl_mut(runtime, |runtime| {
            let result = runtime
                .runtime
                .commit_host_history_state(state)
                .map_err(SnowError::from)?;
            write_changed_viewports(out_changed_viewports, result.changed_viewports);
            Ok(())
        }))
    })
}

/// # Safety
/// If `runtime` is non-null, it must be a live handle returned by `snow_runtime_create`.
/// `out_state` must be valid for writes of one `SnowHistoryState` value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_runtime_get_history_state(
    runtime: SnowRuntime,
    out_state: *mut SnowHistoryState,
) -> SnowError {
    ffi_error(|| {
        if out_state.is_null() {
            return SnowError::InvalidArgument;
        }

        ffi_status(with_runtime_ref(runtime, |runtime| {
            write_out(out_state, runtime.history_state().into());
            Ok(())
        }))
    })
}

/// # Safety
/// If `runtime` is non-null, it must be a live handle returned by `snow_runtime_create`.
/// `out_changed_viewports` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_runtime_clear_document_preserving_viewports(
    runtime: SnowRuntime,
    out_changed_viewports: *mut SnowChangedViewportList,
) -> SnowError {
    ffi_error(|| {
        if out_changed_viewports.is_null() {
            return SnowError::InvalidArgument;
        }

        ffi_status(with_runtime_impl_mut(runtime, |state| {
            let result = state
                .runtime
                .clear_document_preserving_viewports()
                .map_err(SnowError::from)?;
            write_changed_viewports(out_changed_viewports, result.changed_viewports);
            Ok(())
        }))
    })
}

/// # Safety
/// If `runtime` is non-null, it must be a live handle returned by `snow_runtime_create`.
/// `bytes` must point to `size` readable bytes containing a serialized document history.
/// `out_changed_viewports` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_runtime_restore_document_history_preserving_editor_styles(
    runtime: SnowRuntime,
    bytes: *const u8,
    size: usize,
    out_changed_viewports: *mut SnowChangedViewportList,
) -> SnowError {
    ffi_error(|| {
        if bytes.is_null() || size == 0 || out_changed_viewports.is_null() {
            return SnowError::InvalidArgument;
        }
        let bytes = unsafe { std::slice::from_raw_parts(bytes, size) };

        ffi_status(with_runtime_impl_mut(runtime, |state| {
            let result = state
                .runtime
                .restore_document_history_preserving_editor_styles(bytes)
                .map_err(SnowError::from)?;
            write_changed_viewports(out_changed_viewports, result.changed_viewports);
            Ok(())
        }))
    })
}

/// # Safety
/// If `runtime` is non-null, it must be a live handle returned by `snow_runtime_create`.
/// `out_changed_viewports` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_runtime_undo_ex(
    runtime: SnowRuntime,
    out_changed_viewports: *mut SnowChangedViewportList,
) -> SnowError {
    ffi_error(|| {
        if out_changed_viewports.is_null() {
            return SnowError::InvalidArgument;
        }

        ffi_status(with_runtime_impl_mut(runtime, |state| {
            let result = state
                .runtime
                .undo_with_viewport_changes()
                .map_err(SnowError::from)?;
            write_changed_viewports(out_changed_viewports, result.changed_viewports);
            Ok(())
        }))
    })
}

/// # Safety
/// If `runtime` is non-null, it must be a live handle returned by `snow_runtime_create`.
/// `out_changed_viewports` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snow_runtime_redo_ex(
    runtime: SnowRuntime,
    out_changed_viewports: *mut SnowChangedViewportList,
) -> SnowError {
    ffi_error(|| {
        if out_changed_viewports.is_null() {
            return SnowError::InvalidArgument;
        }

        ffi_status(with_runtime_impl_mut(runtime, |state| {
            let result = state
                .runtime
                .redo_with_viewport_changes()
                .map_err(SnowError::from)?;
            write_changed_viewports(out_changed_viewports, result.changed_viewports);
            Ok(())
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    #[test]
    fn host_history_ffi_is_ordered_and_rejects_invalid_arguments() {
        unsafe {
            let mut runtime = std::ptr::null_mut();
            assert_eq!(snow_runtime_create(&mut runtime), SnowError::Ok);
            let mut state = 99;
            let mut changed = std::ptr::null_mut();
            assert_eq!(
                snow_runtime_host_history_state(runtime, 0, &mut state),
                SnowError::Ok
            );
            assert_eq!(state, 0);
            assert_eq!(
                snow_runtime_host_history_state(runtime, 2, &mut state),
                SnowError::InvalidArgument
            );
            assert_eq!(
                snow_runtime_host_history_state(runtime, 0, std::ptr::null_mut()),
                SnowError::InvalidArgument
            );
            assert_eq!(
                snow_runtime_commit_host_history_state(runtime, 1, std::ptr::null_mut()),
                SnowError::InvalidArgument
            );
            assert_eq!(
                snow_runtime_commit_host_history_state(runtime, 1, &mut changed),
                SnowError::Ok
            );
            snow_changed_viewports_destroy(changed);
            assert_eq!(
                snow_runtime_host_history_state(runtime, 0, &mut state),
                SnowError::Ok
            );
            assert_eq!(state, 1);
            assert_eq!(
                snow_runtime_commit_host_history_state(runtime, 1, &mut changed),
                SnowError::InvalidArgument
            );
            assert_eq!(snow_runtime_undo_ex(runtime, &mut changed), SnowError::Ok);
            snow_changed_viewports_destroy(changed);
            assert_eq!(
                snow_runtime_host_history_state(runtime, 0, &mut state),
                SnowError::Ok
            );
            assert_eq!(state, 0);
            assert_eq!(
                snow_runtime_host_history_state(runtime, 1, &mut state),
                SnowError::Ok
            );
            assert_eq!(state, 1);
            assert_eq!(snow_runtime_redo_ex(runtime, &mut changed), SnowError::Ok);
            snow_changed_viewports_destroy(changed);
            assert_eq!(
                snow_runtime_host_history_state(runtime, 0, &mut state),
                SnowError::Ok
            );
            assert_eq!(state, 1);
            snow_runtime_destroy(runtime);
        }
    }
}
