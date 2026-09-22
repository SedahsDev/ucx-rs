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
        // Request address, length, and memory type so UCX fills them in.
        attr.field_mask = ucp_mem_attr_field::UCP_MEM_ATTR_FIELD_ADDRESS as u64
            | ucp_mem_attr_field::UCP_MEM_ATTR_FIELD_LENGTH as u64
            | ucp_mem_attr_field::UCP_MEM_ATTR_FIELD_MEM_TYPE as u64;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context;
    use crate::context::Context;
    use crate::ffi::*;

    // ── MemMapParamsBuilder tests (pure unit tests) ──

    #[test]
    fn test_mem_map_params_builder_new() {
        let builder = MemMapParamsBuilder::new();
        assert_eq!(builder.field_mask, 0u64);
    }

    #[test]
    fn test_mem_map_params_builder_default() {
        let builder: MemMapParamsBuilder = Default::default();
        assert_eq!(builder.field_mask, 0u64);
    }

    #[test]
    fn test_mem_map_params_builder_build_empty() {
        let mut builder = MemMapParamsBuilder::new();
        let params = builder.build();
        assert_eq!(params.handle.field_mask, 0u64);
    }

    #[test]
    fn test_mem_map_params_builder_address() {
        let mut builder = MemMapParamsBuilder::new();
        let data: [u8; 64] = [0; 64];
        builder.address(data.as_ptr() as *mut std::os::raw::c_void);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_ADDRESS as u64)
                != 0
        );
    }

    #[test]
    fn test_mem_map_params_builder_length() {
        let mut builder = MemMapParamsBuilder::new();
        builder.length(4096);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_LENGTH as u64)
                != 0
        );
        assert_eq!(params.handle.length, 4096);
    }

    #[test]
    fn test_mem_map_params_builder_flags() {
        let mut builder = MemMapParamsBuilder::new();
        builder.flags(0);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_FLAGS as u64)
                != 0
        );
    }

    #[test]
    fn test_mem_map_params_builder_prot() {
        let mut builder = MemMapParamsBuilder::new();
        builder.prot(0x3); // PROT_READ | PROT_WRITE
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_PROT as u64)
                != 0
        );
        assert_eq!(params.handle.prot, 0x3);
    }

    #[test]
    fn test_mem_map_params_builder_memory_type() {
        let mut builder = MemMapParamsBuilder::new();
        builder.memory_type(ucs_memory_type_t::UCS_MEMORY_TYPE_HOST);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_MEMORY_TYPE as u64)
                != 0
        );
    }

    #[test]
    fn test_mem_map_params_builder_full_chain() {
        let mut builder = MemMapParamsBuilder::new();
        let data: [u8; 4096] = [0; 4096];
        builder
            .address(data.as_ptr() as *mut std::os::raw::c_void)
            .length(4096)
            .flags(0)
            .prot(0x3)
            .memory_type(ucs_memory_type_t::UCS_MEMORY_TYPE_HOST);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_ADDRESS as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_LENGTH as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_FLAGS as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_PROT as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_MEMORY_TYPE as u64)
                != 0
        );
    }

    #[test]
    fn test_mem_map_params_builder_chaining_returns_mut_ref() {
        let mut builder = MemMapParamsBuilder::new();
        let data: [u8; 128] = [0; 128];
        let result = builder.address(data.as_ptr() as *mut std::os::raw::c_void);
        result.length(128);
        let params = builder.build();
        assert_eq!(params.handle.length, 128);
    }

    // ── MemAdviseParamsBuilder tests (pure unit tests) ──

    #[test]
    fn test_mem_advise_params_builder_new() {
        let builder = MemAdviseParamsBuilder::new();
        assert_eq!(builder.field_mask, 0u64);
    }

    #[test]
    fn test_mem_advise_params_builder_default() {
        let builder: MemAdviseParamsBuilder = Default::default();
        assert_eq!(builder.field_mask, 0u64);
    }

    #[test]
    fn test_mem_advise_params_builder_build_empty() {
        let mut builder = MemAdviseParamsBuilder::new();
        let params = builder.build();
        assert_eq!(params.handle.field_mask, 0u64);
    }

    #[test]
    fn test_mem_advise_params_builder_address() {
        let mut builder = MemAdviseParamsBuilder::new();
        let data: [u8; 64] = [0; 64];
        builder.address(data.as_ptr() as *mut std::os::raw::c_void);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADDRESS as u64)
                != 0
        );
    }

    #[test]
    fn test_mem_advise_params_builder_length() {
        let mut builder = MemAdviseParamsBuilder::new();
        builder.length(2048);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_LENGTH as u64)
                != 0
        );
        assert_eq!(params.handle.length, 2048);
    }

    #[test]
    fn test_mem_advise_params_builder_advice_normal() {
        let mut builder = MemAdviseParamsBuilder::new();
        builder.advice(ucp_mem_advice_t::UCP_MADV_NORMAL);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADVICE as u64)
                != 0
        );
    }

    #[test]
    fn test_mem_advise_params_builder_advice_dont_need() {
        let mut builder = MemAdviseParamsBuilder::new();
        builder.advice(ucp_mem_advice_t::UCP_MADV_WILLNEED);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADVICE as u64)
                != 0
        );
    }

    #[test]
    fn test_mem_advise_params_builder_full_chain() {
        let mut builder = MemAdviseParamsBuilder::new();
        let data: [u8; 1024] = [0; 1024];
        builder
            .address(data.as_ptr() as *mut std::os::raw::c_void)
            .length(1024)
            .advice(ucp_mem_advice_t::UCP_MADV_NORMAL);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADDRESS as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_LENGTH as u64)
                != 0
        );
        assert!(
            params.handle.field_mask
                & (ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADVICE as u64)
                != 0
        );
    }

    // ── MemhPackParamsBuilder tests (pure unit tests) ──

    #[test]
    fn test_memh_pack_params_builder_new() {
        let builder = MemhPackParamsBuilder::new();
        assert_eq!(builder.field_mask, 0u64);
    }

    #[test]
    fn test_memh_pack_params_builder_default() {
        let builder: MemhPackParamsBuilder = Default::default();
        assert_eq!(builder.field_mask, 0u64);
    }

    #[test]
    fn test_memh_pack_params_builder_build_empty() {
        let mut builder = MemhPackParamsBuilder::new();
        let params = builder.build();
        assert_eq!(params.handle.field_mask, 0u64);
    }

    #[test]
    fn test_memh_pack_params_builder_flags() {
        let mut builder = MemhPackParamsBuilder::new();
        builder.flags(0);
        let params = builder.build();
        assert!(
            params.handle.field_mask
                & (ucp_memh_pack_params_field::UCP_MEMH_PACK_PARAM_FIELD_FLAGS as u64)
                != 0
        );
    }

    // ── PackedMemhBuffer tests (pure unit tests on empty buffer) ──

    #[test]
    fn test_packed_memh_buffer_empty_constructor() {
        // Create a buffer with null pointer to test empty methods
        let buf = PackedMemhBuffer {
            buffer: std::ptr::null_mut(),
            length: 0,
        };
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert!(buf.as_bytes().is_empty());
        assert!(buf.as_ptr().is_null());
    }

    // ── PackedRkeyBuffer tests (pure unit tests on empty buffer) ──

    #[test]
    fn test_packed_rkey_buffer_empty_constructor() {
        let buf = PackedRkeyBuffer {
            buffer: std::ptr::null_mut(),
            size: 0,
        };
        assert_eq!(buf.size(), 0);
        assert!(buf.as_bytes().is_empty());
        assert!(buf.as_ptr().is_null());
    }

    // ── MemHandle integration tests (require UCX library) ──

    #[test]
    fn test_mem_handle_map_and_query() {
        let config = context::Config::default();
        let ctx_params = context::ParamsBuilder::new()
            .features(context::Flags::Rma)
            .build();
        let ctx = Context::new(&config, &ctx_params).expect("context init");

        let data: Vec<u8> = vec![0xAB; 4096];
        let mut builder = MemMapParamsBuilder::new();
        builder
            .address(data.as_ptr() as *mut std::os::raw::c_void)
            .length(4096);

        let memh = MemHandle::map(&ctx, &mut builder).expect("mem_map");
        assert!(!memh.as_raw().is_null());

        // Query just needs to succeed — ucp_mem_query returns basic attrs
        // Note: length/mem_type may be 0 if field_mask wasn't set by caller
        let _attr = memh.query().expect("mem_query");
    }

    #[test]
    fn test_mem_handle_map_and_pack_rkey() {
        let config = context::Config::default();
        let ctx_params = context::ParamsBuilder::new()
            .features(context::Flags::Rma)
            .build();
        let ctx = Context::new(&config, &ctx_params).expect("context init");

        let data: Vec<u8> = vec![0xCD; 4096];
        let mut builder = MemMapParamsBuilder::new();
        builder
            .address(data.as_ptr() as *mut std::os::raw::c_void)
            .length(4096);

        let memh = MemHandle::map(&ctx, &mut builder).expect("mem_map");

        let rkey = pack_rkey(&ctx, &memh).expect("pack_rkey");
        assert!(!rkey.as_ptr().is_null());
        assert!(rkey.size() > 0);
        assert!(!rkey.as_bytes().is_empty());
    }

    #[test]
    fn test_mem_handle_as_raw() {
        let config = context::Config::default();
        let ctx_params = context::ParamsBuilder::new()
            .features(context::Flags::Rma)
            .build();
        let ctx = Context::new(&config, &ctx_params).expect("context init");

        let data: Vec<u8> = vec![0; 1024];
        let mut builder = MemMapParamsBuilder::new();
        builder
            .address(data.as_ptr() as *mut std::os::raw::c_void)
            .length(1024);

        let memh = MemHandle::map(&ctx, &mut builder).expect("mem_map");
        let raw = memh.as_raw();
        assert!(!raw.is_null());
    }

    // ── MemMapParamsBuilder field mask tests ──

    #[test]
    fn test_mem_map_field_address_is_bit_0() {
        assert_eq!(
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_ADDRESS as u64,
            1
        );
    }

    #[test]
    fn test_mem_map_field_length_is_bit_1() {
        assert_eq!(
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_LENGTH as u64,
            2
        );
    }

    #[test]
    fn test_mem_map_field_flags_is_bit_2() {
        assert_eq!(
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_FLAGS as u64,
            4
        );
    }

    #[test]
    fn test_mem_map_field_prot_is_bit_3() {
        assert_eq!(
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_PROT as u64,
            8
        );
    }

    #[test]
    fn test_mem_map_field_memory_type_is_bit_4() {
        assert_eq!(
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_MEMORY_TYPE as u64,
            16
        );
    }

    #[test]
    fn test_mem_map_field_exported_memh_buffer_is_bit_5() {
        assert_eq!(
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_EXPORTED_MEMH_BUFFER as u64,
            32
        );
    }

    #[test]
    fn test_mem_map_fields_all_distinct() {
        let fields = [
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_ADDRESS as u64,
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_LENGTH as u64,
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_FLAGS as u64,
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_PROT as u64,
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_MEMORY_TYPE as u64,
            ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_EXPORTED_MEMH_BUFFER as u64,
        ];
        for i in 0..fields.len() {
            for j in (i + 1)..fields.len() {
                assert_eq!(
                    fields[i] & fields[j],
                    0,
                    "Fields {} and {} should be distinct",
                    i,
                    j
                );
            }
        }
    }

    // ── MemAdviseParamsBuilder field mask tests ──

    #[test]
    fn test_mem_advise_field_address_is_bit_0() {
        assert_eq!(
            ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADDRESS as u64,
            1
        );
    }

    #[test]
    fn test_mem_advise_field_length_is_bit_1() {
        assert_eq!(
            ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_LENGTH as u64,
            2
        );
    }

    #[test]
    fn test_mem_advise_field_advice_is_bit_2() {
        assert_eq!(
            ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADVICE as u64,
            4
        );
    }

    #[test]
    fn test_mem_advise_fields_all_distinct() {
        let fields = [
            ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADDRESS as u64,
            ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_LENGTH as u64,
            ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADVICE as u64,
        ];
        for i in 0..fields.len() {
            for j in (i + 1)..fields.len() {
                assert_eq!(
                    fields[i] & fields[j],
                    0,
                    "Fields {} and {} should be distinct",
                    i,
                    j
                );
            }
        }
    }
}
