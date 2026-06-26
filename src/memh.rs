//! UCP memory handle bindings.
//!
//! Wraps `ucp_mem_map`, `ucp_mem_unmap`, `ucp_mem_query`, `ucp_mem_advise`,
//! `ucp_memh_pack`, `ucp_rkey_pack`, and their release functions.

use crate::context::Context;
use crate::ffi::*;
use crate::status_to_result;

/// RAII wrapper around a UCP memory handle (`ucp_mem_h`).
/// The handle is automatically unmapped when dropped.
pub struct MemHandle {
    context: ucp_context_h,
    handle: ucp_mem_h,
}

impl MemHandle {
    /// Map or register memory with the given parameters.
    pub fn map(
        context: &Context,
        params: &mut MemMapParamsBuilder,
    ) -> Result<MemHandle, ucs_status_t> {
        let built = params.build();
        let mut memh: ucp_mem_h = std::ptr::null_mut();
        let result =
            status_to_result(unsafe { ucp_mem_map(context.handle, &built.handle, &mut memh) });
        match result {
            Ok(()) => Ok(MemHandle {
                context: context.handle,
                handle: memh,
            }),
            Err(e) => Err(e),
        }
    }

    /// Query attributes of this memory handle.
    pub fn query(&self) -> Result<MemAttr, ucs_status_t> {
        let mut attr: ucp_mem_attr_t = unsafe { std::mem::zeroed() };
        let result = status_to_result(unsafe { ucp_mem_query(self.handle, &mut attr) });
        match result {
            Ok(()) => Ok(MemAttr { handle: attr }),
            Err(e) => Err(e),
        }
    }

    /// Give advice about how the application will access the memory region.
    pub fn advise(&self, params: &mut MemAdviseParamsBuilder) -> Result<(), ucs_status_t> {
        let mut built = params.build();
        status_to_result(unsafe { ucp_mem_advise(self.context, self.handle, &mut built.handle) })
    }

    /// Get the raw UCP memory handle.
    #[inline]
    pub fn as_raw(&self) -> ucp_mem_h {
        self.handle
    }
}

impl Drop for MemHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = ucp_mem_unmap(self.context, self.handle);
        };
    }
}

// ── Memory Map Params Builder ──

/// Builder for `ucp_mem_map_params_t`.
pub struct MemMapParamsBuilder {
    uninit_handle: std::mem::MaybeUninit<ucp_mem_map_params_t>,
    field_mask: u64,
}

impl Default for MemMapParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MemMapParamsBuilder {
    pub fn new() -> Self {
        Self {
            uninit_handle: std::mem::MaybeUninit::uninit(),
            field_mask: 0,
        }
    }

    /// Set the address to map (or null for library-allocated memory).
    pub fn address(&mut self, addr: *mut std::os::raw::c_void) -> &mut Self {
        self.field_mask |= ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_ADDRESS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.address = addr;
        self
    }

    /// Set the length in bytes (mandatory).
    pub fn length(&mut self, len: usize) -> &mut Self {
        self.field_mask |= ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_LENGTH as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.length = len;
        self
    }

    /// Set allocation flags.
    pub fn flags(&mut self, flags: u32) -> &mut Self {
        self.field_mask |= ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_FLAGS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.flags = flags;
        self
    }

    /// Set memory protection mode.
    pub fn prot(&mut self, prot: u32) -> &mut Self {
        self.field_mask |= ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_PROT as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.prot = prot;
        self
    }

    /// Set memory type.
    pub fn memory_type(&mut self, mem_type: ucs_memory_type_t) -> &mut Self {
        self.field_mask |= ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_MEMORY_TYPE as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.memory_type = mem_type;
        self
    }

    /// Set exported memory handle buffer.
    pub fn exported_memh_buffer(&mut self, buffer: *const std::os::raw::c_void) -> &mut Self {
        self.field_mask |=
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_EXPORTED_MEMH_BUFFER as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.exported_memh_buffer = buffer;
        self
    }

    pub fn build(&mut self) -> MemMapParams {
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.field_mask = self.field_mask;
        MemMapParams {
            handle: unsafe { self.uninit_handle.assume_init() },
        }
    }
}

/// Built memory map parameters.
pub struct MemMapParams {
    handle: ucp_mem_map_params_t,
}

// ── Memory Advice Params Builder ──

/// Builder for `ucp_mem_advise_params_t`.
pub struct MemAdviseParamsBuilder {
    uninit_handle: std::mem::MaybeUninit<ucp_mem_advise_params_t>,
    field_mask: u64,
}

impl Default for MemAdviseParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MemAdviseParamsBuilder {
    pub fn new() -> Self {
        Self {
            uninit_handle: std::mem::MaybeUninit::uninit(),
            field_mask: 0,
        }
    }

    pub fn address(&mut self, addr: *mut std::os::raw::c_void) -> &mut Self {
        self.field_mask |= ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADDRESS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.address = addr;
        self
    }

    pub fn length(&mut self, len: usize) -> &mut Self {
        self.field_mask |= ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_LENGTH as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.length = len;
        self
    }

    pub fn advice(&mut self, advice: ucp_mem_advice_t) -> &mut Self {
        self.field_mask |= ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADVICE as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.advice = advice;
        self
    }

    pub fn build(&mut self) -> MemAdviseParams {
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.field_mask = self.field_mask;
        MemAdviseParams {
            handle: unsafe { self.uninit_handle.assume_init() },
        }
    }
}

/// Built memory advice parameters.
pub struct MemAdviseParams {
    handle: ucp_mem_advise_params_t,
}

// ── Memory Handle Pack (ucp_memh_pack) ──

/// Builder for `ucp_memh_pack_params_t`.
pub struct MemhPackParamsBuilder {
    uninit_handle: std::mem::MaybeUninit<ucp_memh_pack_params_t>,
    field_mask: u64,
}

impl Default for MemhPackParamsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MemhPackParamsBuilder {
    pub fn new() -> Self {
        Self {
            uninit_handle: std::mem::MaybeUninit::uninit(),
            field_mask: 0,
        }
    }

    pub fn flags(&mut self, flags: u64) -> &mut Self {
        self.field_mask |= ucp_memh_pack_params_field::UCP_MEMH_PACK_PARAM_FIELD_FLAGS as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.flags = flags;
        self
    }

    pub fn build(&mut self) -> MemhPackParams {
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.field_mask = self.field_mask;
        MemhPackParams {
            handle: unsafe { self.uninit_handle.assume_init() },
        }
    }
}

/// Built memory handle pack parameters.
pub struct MemhPackParams {
    handle: ucp_memh_pack_params_t,
}

/// Pack a memory handle into an exportable buffer.
///
/// The returned buffer is automatically released when dropped.
pub fn pack_memh(
    memh: &MemHandle,
    params: &mut MemhPackParamsBuilder,
) -> Result<PackedMemhBuffer, ucs_status_t> {
    let built = params.build();
    let mut buffer: *mut std::os::raw::c_void = std::ptr::null_mut();
    let mut length: usize = 0;
    let result = status_to_result(unsafe {
        ucp_memh_pack(memh.handle, &built.handle, &mut buffer, &mut length)
    });
    match result {
        Ok(()) => Ok(PackedMemhBuffer { buffer, length }),
        Err(e) => Err(e),
    }
}

/// RAII wrapper for a packed memory handle buffer.
pub struct PackedMemhBuffer {
    buffer: *mut std::os::raw::c_void,
    length: usize,
}

impl PackedMemhBuffer {
    /// Get a pointer to the buffer data.
    #[inline]
    pub fn as_ptr(&self) -> *const std::os::raw::c_void {
        self.buffer as *const _
    }

    /// Get the buffer length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.length
    }

    /// Check if the buffer is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Get the buffer contents as a safe byte slice.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        if self.buffer.is_null() || self.length == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.buffer as *const u8, self.length) }
    }
}

impl Drop for PackedMemhBuffer {
    fn drop(&mut self) {
        if !self.buffer.is_null() {
            let params: ucp_memh_buffer_release_params_t = unsafe { std::mem::zeroed() };
            unsafe { ucp_memh_buffer_release(self.buffer, &params) };
            self.buffer = std::ptr::null_mut();
        }
    }
}

// ── Legacy rkey pack (ucp_rkey_pack) ──

/// Pack a memory handle into an rkey buffer using the legacy `ucp_rkey_pack` API.
///
/// This works on all memory domains (including `self`, `sysv`, `posix`) unlike
/// `ucp_memh_pack` which requires the memory domain to support `pack_rkey`.
/// The returned buffer is automatically released when dropped.
pub fn pack_rkey(context: &Context, memh: &MemHandle) -> Result<PackedRkeyBuffer, ucs_status_t> {
    let mut buffer: *mut std::os::raw::c_void = std::ptr::null_mut();
    let mut size: usize = 0;
    let result = status_to_result(unsafe {
        ucp_rkey_pack(context.handle, memh.handle, &mut buffer, &mut size)
    });
    match result {
        Ok(()) => Ok(PackedRkeyBuffer { buffer, size }),
        Err(e) => Err(e),
    }
}

/// RAII wrapper for a packed rkey buffer from `ucp_rkey_pack`.
pub struct PackedRkeyBuffer {
    buffer: *mut std::os::raw::c_void,
    size: usize,
}

impl PackedRkeyBuffer {
    /// Get a pointer to the buffer data.
    #[inline]
    pub fn as_ptr(&self) -> *const std::os::raw::c_void {
        self.buffer as *const _
    }

    /// Get the buffer size in bytes.
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Get the buffer contents as a safe byte slice.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        if self.buffer.is_null() || self.size == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.buffer as *const u8, self.size) }
    }
}

impl Drop for PackedRkeyBuffer {
    fn drop(&mut self) {
        if !self.buffer.is_null() {
            unsafe { ucp_rkey_buffer_release(self.buffer) };
            self.buffer = std::ptr::null_mut();
        }
    }
}

// ── Memory Attributes ──

/// Attributes returned by `MemHandle::query()`.
pub struct MemAttr {
    handle: ucp_mem_attr_t,
}

impl MemAttr {
    /// Get the address of the mapped memory region.
    #[inline]
    pub fn address(&self) -> *mut std::os::raw::c_void {
        self.handle.address
    }

    /// Get the length of the mapped memory region.
    #[inline]
    pub fn length(&self) -> usize {
        self.handle.length
    }

    /// Get the memory type.
    #[inline]
    pub fn mem_type(&self) -> ucs_memory_type_t {
        self.handle.mem_type
    }
}
