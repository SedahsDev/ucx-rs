//! UCP memory handle bindings.
//!
//! Wraps `ucp_mem_map`, `ucp_mem_unmap`, `ucp_mem_query`, `ucp_mem_advise`,
//! `ucp_memh_pack`, `ucp_rkey_pack`, and their release functions.

use crate::context::Context;
use crate::ffi::*;
use crate::status_to_result;
use crate::Status;
use crate::{OwnedBuf, OwnedBufMut};
use std::ffi::CString;
use std::marker::PhantomData;
use std::os::fd::RawFd;

/// Print UCX memory diagnostics for `mem_spec` to `fd`.
pub fn mem_print_info(
    mem_spec: &str,
    context: &Context,
    fd: RawFd,
) -> Result<(), std::ffi::NulError> {
    let spec = CString::new(mem_spec)?;
    let _ = crate::config::with_file_stream(fd, |stream| {
        // SAFETY: context is a live UCX handle; spec and stream are valid.
        unsafe { ucp_mem_print_info(spec.as_ptr(), context.handle, stream.cast()) };
    });
    Ok(())
}

/// RAII wrapper around a UCP memory handle (`ucp_mem_h`).
/// The handle is automatically unmapped when dropped.
pub struct MemHandle {
    context: ucp_context_h,
    handle: ucp_mem_h,
}

/// A registered buffer whose borrow lasts until the UCX memory handle is unmapped.
pub struct MemHandleGuard<'a> {
    memh: MemHandle,
    _buffer: PhantomData<&'a mut [u8]>,
}

/// Memory advice values for `MemHandle::advice()`.
///
/// These map to the UCX `ucp_mem_advice_t` enum but are defined natively
/// to avoid exposing FFI types in the public API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemAdvice {
    /// Normal memory access pattern.
    Normal,
    /// Memory will be needed soon.
    WillNeed,
}

impl MemAdvice {
    /// Convert to the corresponding UCX FFI type.
    pub(crate) fn to_ffi(self) -> crate::ffi::ucp_mem_advice_t {
        match self {
            MemAdvice::Normal => crate::ffi::ucp_mem_advice_t::UCP_MADV_NORMAL,
            MemAdvice::WillNeed => crate::ffi::ucp_mem_advice_t::UCP_MADV_WILLNEED,
        }
    }
}

impl<'a> MemHandleGuard<'a> {
    /// Access the underlying registered memory handle.
    pub fn mem_handle(&self) -> &MemHandle {
        &self.memh
    }
}

impl MemHandle {
    /// Map a caller-owned buffer and keep it borrowed for the guard's lifetime.
    ///
    /// Unlike [`Self::map`] with [`MemMapParamsBuilder::address`], this API makes
    /// it impossible to drop or mutably reuse the buffer while UCX has it
    /// registered. The guard unmaps the memory when dropped.
    pub fn map_slice<'a>(
        context: &Context,
        mem: &'a mut [u8],
        flags: u64,
    ) -> Result<MemHandleGuard<'a>, Status> {
        let flags = u32::try_from(flags).map_err(|_| ucs_status_t::UCS_ERR_INVALID_PARAM)?;
        let mut params = MemMapParamsBuilder::new();
        params.address_slice(mem).flags(flags);
        Self::map(context, &mut params).map(|memh| MemHandleGuard {
            memh,
            _buffer: PhantomData,
        })
    }

    /// Map or register memory with the given parameters.
    pub fn map(context: &Context, params: &mut MemMapParamsBuilder) -> Result<MemHandle, Status> {
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
    pub fn query(&self) -> Result<MemAttr, Status> {
        let mut attr: ucp_mem_attr_t = unsafe { std::mem::zeroed() };
        // UCX fills only the fields named in `field_mask`; with it zero,
        // every attribute read back as 0.
        attr.field_mask = ucp_mem_attr_field::UCP_MEM_ATTR_FIELD_ADDRESS as u64
            | ucp_mem_attr_field::UCP_MEM_ATTR_FIELD_LENGTH as u64
            | ucp_mem_attr_field::UCP_MEM_ATTR_FIELD_MEM_TYPE as u64;
        let result = status_to_result(unsafe { ucp_mem_query(self.handle, &mut attr) });
        match result {
            Ok(()) => Ok(MemAttr::from_ffi(attr)),
            Err(e) => Err(e),
        }
    }

    /// Give advice about how the application will access the memory region.
    pub fn advise(&self, params: &mut MemAdviseParamsBuilder) -> Result<(), Status> {
        let mut built = params.build();
        status_to_result(unsafe { ucp_mem_advise(self.context, self.handle, &mut built.handle) })
    }

    /// Get the raw UCP memory handle.
    #[inline]
    pub fn as_raw(&self) -> ucp_mem_h {
        self.handle
    }

    /// Check that this handle can be passed to UCX (`UCP_OP_ATTR_FIELD_MEMH`)
    /// for an operation on `worker` over `len` bytes at `ptr` of type
    /// `memory_type`: same context as the worker, the whole range inside the
    /// mapped region, and the same memory type unless `memory_type` is
    /// `Unknown`. `UCS_ERR_INVALID_PARAM` otherwise.
    ///
    /// UCX itself checks the range only in builds with parameter checking, and
    /// a handle from another context would index the wrong memory domains.
    pub(crate) fn check_covers(
        &self,
        worker: &crate::worker::Worker,
        ptr: *const u8,
        len: usize,
        memory_type: crate::MemoryType,
    ) -> Result<(), Status> {
        if self.context != worker.context {
            return Err(Status::INVALID_PARAM);
        }
        let attr = self.query()?;
        let start = attr.address() as usize;
        let end = start.checked_add(attr.length());
        let ptr = ptr as usize;
        let covered = match (end, ptr.checked_add(len)) {
            (Some(end), Some(ptr_end)) => ptr >= start && ptr_end <= end,
            _ => false,
        };
        let type_matches =
            memory_type == crate::MemoryType::Unknown || memory_type == attr.mem_type();
        if covered && type_matches {
            Ok(())
        } else {
            Err(Status::INVALID_PARAM)
        }
    }
}

/// A buffer kept registered with UCX for as long as it is owned (#210).
///
/// [`RegisteredBuf::new`] maps `B`'s storage with `ucp_mem_map`. The handle
/// and the buffer travel together: every operation passes the handle to UCX
/// (`UCP_OP_ATTR_FIELD_MEMH`), so UCX does not register (or look up) the
/// buffer per operation, and the handle cannot be unmapped while an operation
/// that uses it is in flight, because the [`crate::OwningRequest`] owns both.
/// Dropping a `RegisteredBuf` unmaps the handle first, then drops `B`.
///
/// It is an [`OwnedBuf`] (and an [`OwnedBufMut`] if `B` is), so it works with
/// every owned operation and the same [`crate::Completion`] /
/// [`crate::OwningRequest`] types as a plain `Vec<u8>`:
///
/// ```no_run
/// # use ucx_sys::{context::Context, ep::Ep, worker::Worker, memh::RegisteredBuf};
/// # use ucx_sys::{Completion, RequestParamBuilder};
/// # fn demo(context: &Context, ep: &Ep, worker: &Worker) {
/// let buffer = RegisteredBuf::new(context, vec![7u8; 1 << 20]).expect("ucp_mem_map");
/// let param = RequestParamBuilder::new().build();
/// match ep.tag_send(worker, buffer, 0x42, &param).unwrap() {
///     Completion::Immediate(buffer) => drop(buffer),
///     Completion::Pending(request) => { /* worker.progress() + request.try_complete() */ }
/// }
/// # }
/// ```
///
/// The registration must be made on the context of the worker the buffer is
/// used with; otherwise operations fail with `UCS_ERR_INVALID_PARAM`.
pub struct RegisteredBuf<B: OwnedBuf> {
    // Declared before `buffer`: fields drop in order, so the memory is
    // unmapped before the buffer is freed.
    memh: MemHandle,
    buffer: B,
    memory_type: crate::MemoryType,
}

impl<B: OwnedBuf> RegisteredBuf<B> {
    /// Register `buffer`'s storage with `context`.
    ///
    /// The memory type passed to `ucp_mem_map` is `buffer.memory_type()`
    /// (UCX detects it if `Unknown`); afterwards the type UCX recorded is the
    /// one reported. On failure (including an empty buffer, which UCX cannot
    /// map) the buffer comes back inside [`crate::Failed`].
    pub fn new(context: &Context, buffer: B) -> Result<Self, crate::Failed<B>> {
        if buffer.is_empty() {
            return Err(crate::Failed {
                status: Status::INVALID_PARAM,
                data: buffer,
            });
        }
        let mut params = MemMapParamsBuilder::new();
        params
            .address(buffer.as_ptr() as *mut std::os::raw::c_void)
            .length(buffer.len())
            .memory_type(buffer.memory_type());
        let memh = match MemHandle::map(context, &mut params) {
            Ok(memh) => memh,
            Err(status) => {
                return Err(crate::Failed {
                    status,
                    data: buffer,
                })
            }
        };
        let memory_type = match memh.query() {
            Ok(attr) => attr.mem_type(),
            Err(status) => {
                drop(memh);
                return Err(crate::Failed {
                    status,
                    data: buffer,
                });
            }
        };
        Ok(Self {
            memh,
            buffer,
            memory_type,
        })
    }

    /// The registered buffer. Only shared access: mutating it through its own
    /// API (e.g. growing a `Vec`) could move the storage out of the mapping.
    pub fn get_ref(&self) -> &B {
        &self.buffer
    }

    /// The memory handle.
    pub fn mem_handle(&self) -> &MemHandle {
        &self.memh
    }

    /// Unmap the memory and return the buffer.
    pub fn into_inner(self) -> B {
        let Self { memh, buffer, .. } = self;
        drop(memh);
        buffer
    }
}

impl<B: OwnedBuf> std::fmt::Debug for RegisteredBuf<B> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegisteredBuf")
            .field("len", &self.buffer.len())
            .field("memory_type", &self.memory_type)
            .finish_non_exhaustive()
    }
}

// SAFETY: delegates the storage to `B` (an `OwnedBuf`), and reports the
// memory type UCX recorded when it mapped exactly that storage.
unsafe impl<B: OwnedBuf> OwnedBuf for RegisteredBuf<B> {
    fn as_ptr(&self) -> *const u8 {
        self.buffer.as_ptr()
    }
    fn len(&self) -> usize {
        self.buffer.len()
    }
    fn memory_type(&self) -> crate::MemoryType {
        self.memory_type
    }
    fn memh(&self) -> Option<&MemHandle> {
        Some(&self.memh)
    }
}

// SAFETY: as for `OwnedBuf`; `B: OwnedBufMut` grants the write access.
unsafe impl<B: OwnedBufMut> OwnedBufMut for RegisteredBuf<B> {
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.buffer.as_mut_ptr()
    }
}

/// Memory allocated by UCX (`ucp_mem_map` with `UCP_MEM_MAP_ALLOCATE`) and
/// registered for its whole life, e.g. CUDA device memory (#210).
///
/// An [`OwnedBuf`] / [`OwnedBufMut`] that reports the memory type UCX
/// allocated and its own memory handle, so device memory goes through the
/// same safe, owned operations and [`crate::Completion`] types as host
/// buffers. Its contents are only reachable through UCX operations (the host
/// may not be able to dereference it, and it starts uninitialized), so there
/// is no slice accessor: fill it with a receive or get, read it out with a
/// send or put. Dropping it unmaps and frees the memory.
///
/// ```no_run
/// # use ucx_sys::{context::Context, memh::AllocatedBuf, MemoryType};
/// # fn demo(context: &Context) {
/// let device = AllocatedBuf::new(context, 1 << 20, MemoryType::Cuda).expect("CUDA-enabled UCX");
/// assert_eq!(device.memory_type(), MemoryType::Cuda);
/// # }
/// ```
pub struct AllocatedBuf {
    memh: MemHandle,
    ptr: *mut u8,
    len: usize,
    memory_type: crate::MemoryType,
}

impl AllocatedBuf {
    /// Allocate `len` bytes of `memory_type` memory through UCX.
    ///
    /// `MemoryType::Unknown` lets UCX choose (host memory). If UCX allocates
    /// a different type than requested (it falls back between its allocation
    /// methods), the memory is freed again and this fails with
    /// `UCS_ERR_UNSUPPORTED`; a UCX built without CUDA support fails here for
    /// `MemoryType::Cuda`. `len` must be non-zero (`UCS_ERR_INVALID_PARAM`).
    pub fn new(
        context: &Context,
        len: usize,
        memory_type: crate::MemoryType,
    ) -> Result<Self, Status> {
        if len == 0 {
            return Err(Status::INVALID_PARAM);
        }
        let mut params = MemMapParamsBuilder::new();
        params
            .address(std::ptr::null_mut())
            .length(len)
            .flags(ucp_mem_map_flags_allocate())
            .memory_type(memory_type);
        let memh = MemHandle::map(context, &mut params)?;
        let attr = memh.query()?;
        let allocated = attr.mem_type();
        if attr.address().is_null()
            || attr.length() < len
            || (memory_type != crate::MemoryType::Unknown && allocated != memory_type)
        {
            return Err(Status::UNSUPPORTED);
        }
        Ok(Self {
            ptr: attr.address() as *mut u8,
            len,
            memory_type: allocated,
            memh,
        })
    }

    /// The memory type UCX allocated.
    pub fn memory_type(&self) -> crate::MemoryType {
        self.memory_type
    }

    /// The memory handle.
    pub fn mem_handle(&self) -> &MemHandle {
        &self.memh
    }
}

impl std::fmt::Debug for AllocatedBuf {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AllocatedBuf")
            .field("ptr", &self.ptr)
            .field("len", &self.len)
            .field("memory_type", &self.memory_type)
            .finish_non_exhaustive()
    }
}

// SAFETY: the allocation is owned by `memh` (freed only when it is unmapped
// in `Drop`), never moves, covers at least `len` bytes, and `memory_type` is
// the type UCX reported for it.
unsafe impl OwnedBuf for AllocatedBuf {
    fn as_ptr(&self) -> *const u8 {
        self.ptr
    }
    fn len(&self) -> usize {
        self.len
    }
    fn memory_type(&self) -> crate::MemoryType {
        self.memory_type
    }
    fn memh(&self) -> Option<&MemHandle> {
        Some(&self.memh)
    }
}

// SAFETY: as for `OwnedBuf`; UCX-allocated memory is writable.
unsafe impl OwnedBufMut for AllocatedBuf {
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }
}

/// `UCP_MEM_MAP_ALLOCATE` as the `u32` [`MemMapParamsBuilder::flags`] takes.
fn ucp_mem_map_flags_allocate() -> u32 {
    UCP_MEM_MAP_ALLOCATE as u32
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
            // SAFETY: UCX parameter structs are valid when zeroed.
            uninit_handle: std::mem::MaybeUninit::new(unsafe { std::mem::zeroed() }),
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

    /// Set the address and length from a borrowed slice.
    ///
    /// The builder itself does not retain the borrow; use [`MemHandle::map_slice`]
    /// when mapping caller-owned memory so the resulting guard carries it.
    pub fn address_slice(&mut self, slice: &mut [u8]) -> &mut Self {
        self.address(slice.as_mut_ptr() as *mut std::os::raw::c_void)
            .length(slice.len())
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

    /// Set the memory type of the region.
    ///
    /// When registering existing memory (see `address`), the memory type is a hint that spares
    /// UCX from detecting it. Leaving it unset, or passing `MemoryType::Unknown`, makes UCX
    /// detect the memory type itself. When UCX allocates the memory (the allocate flag passed
    /// to `flags`), this selects the type of memory to allocate; if it is unset, UCX allocates
    /// host memory.
    pub fn memory_type(&mut self, mem_type: crate::MemoryType) -> &mut Self {
        self.field_mask |= ucp_mem_map_params_field::UCP_MEM_MAP_PARAM_FIELD_MEMORY_TYPE as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.memory_type = mem_type.to_ffi();
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
            // SAFETY: UCX parameter structs are valid when zeroed.
            uninit_handle: std::mem::MaybeUninit::new(unsafe { std::mem::zeroed() }),
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

    pub fn advice(&mut self, advice: MemAdvice) -> &mut Self {
        self.field_mask |= ucp_mem_advise_params_field::UCP_MEM_ADVISE_PARAM_FIELD_ADVICE as u64;
        let params = unsafe { &mut *self.uninit_handle.as_mut_ptr() };
        params.advice = advice.to_ffi();
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
            // SAFETY: UCX parameter structs are valid when zeroed.
            uninit_handle: std::mem::MaybeUninit::new(unsafe { std::mem::zeroed() }),
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
) -> Result<PackedMemhBuffer, Status> {
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
pub fn pack_rkey(context: &Context, memh: &MemHandle) -> Result<PackedRkeyBuffer, Status> {
    let mut buffer: *mut std::os::raw::c_void = std::ptr::null_mut();
    let mut size: usize = 0;
    let result = status_to_result(unsafe {
        ucp_rkey_pack(context.handle, memh.handle, &mut buffer, &mut size)
    });
    match result {
        Ok(()) => match PackedRkeyBuffer::from_raw_parts(buffer, size) {
            Ok(packed) => Ok(packed),
            Err(error) => {
                unsafe { ucp_rkey_buffer_release(buffer) };
                Err(error)
            }
        },
        Err(e) => Err(e),
    }
}

/// RAII wrapper for a packed rkey buffer from `ucp_rkey_pack`.
///
/// `as_bytes()` exposes the canonical framed wire representation
/// `[4-byte little-endian payload length][opaque UCX payload]`, which can be
/// passed directly to `RemoteKey::unpack`.
pub struct PackedRkeyBuffer {
    buffer: *mut std::os::raw::c_void,
    size: usize,
    framed: Vec<u8>,
}

impl PackedRkeyBuffer {
    fn from_raw_parts(buffer: *mut std::os::raw::c_void, size: usize) -> Result<Self, Status> {
        let payload = unsafe { std::slice::from_raw_parts(buffer as *const u8, size) };
        let payload_len = u32::try_from(size).map_err(|_| ucs_status_t::UCS_ERR_OUT_OF_RANGE)?;
        let mut framed = Vec::with_capacity(4 + size);
        framed.extend_from_slice(&payload_len.to_le_bytes());
        framed.extend_from_slice(payload);
        Ok(Self {
            buffer,
            size,
            framed,
        })
    }

    /// Get a pointer to the raw UCX payload.
    #[inline]
    pub fn as_ptr(&self) -> *const std::os::raw::c_void {
        self.buffer as *const _
    }

    /// Get the raw UCX payload size in bytes.
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Get the canonical wire bytes as `[4-byte little-endian payload length][payload]`.
    /// These are the bytes accepted by `RemoteKey::unpack`; use this accessor for
    /// sending a packed key to a peer. The raw UCX payload remains available via
    /// `as_ptr` and `size` for direct FFI use.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.framed
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
    address: *mut std::os::raw::c_void,
    length: usize,
    mem_type: ucs_memory_type_t,
}

impl MemAttr {
    /// Construct a `MemAttr` from the raw FFI struct.
    pub(crate) fn from_ffi(ffi: ucp_mem_attr_t) -> Self {
        MemAttr {
            address: ffi.address,
            length: ffi.length,
            mem_type: ffi.mem_type,
        }
    }

    /// Get the address of the mapped memory region.
    #[inline]
    pub fn address(&self) -> *mut std::os::raw::c_void {
        self.address
    }

    /// Get the length of the mapped memory region.
    #[inline]
    pub fn length(&self) -> usize {
        self.length
    }

    /// Get the memory type.
    #[inline]
    pub fn mem_type(&self) -> crate::MemoryType {
        crate::MemoryType::from_ffi(self.mem_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_rkey_as_bytes_is_framed_wire_data() {
        let packed = PackedRkeyBuffer {
            buffer: std::ptr::null_mut(),
            size: 3,
            framed: vec![3, 0, 0, 0, 0xaa, 0xbb, 0xcc],
        };
        assert_eq!(packed.as_bytes(), &[3, 0, 0, 0, 0xaa, 0xbb, 0xcc]);
        assert_eq!(&packed.as_bytes()[4..], &[0xaa, 0xbb, 0xcc]);
    }

    #[test]
    fn map_slice_signature_keeps_guard_borrowed() {
        let _: for<'a> fn(&Context, &'a mut [u8], u64) -> Result<MemHandleGuard<'a>, Status> =
            MemHandle::map_slice;
    }

    /// `query` must request the fields it reports: with a zero `field_mask`
    /// UCX fills nothing and every attribute reads back as zero.
    #[test]
    fn query_reports_mapped_region() {
        use crate::context::{Config, Context, Flags, ParamsBuilder};
        let context = Context::new(
            &Config::read("", "").expect("config"),
            &ParamsBuilder::new()
                .features(Flags::Rma)
                .mt_workers_shared(1)
                .estimated_num_eps(1)
                .build(),
        )
        .expect("context");
        let mut buffer = vec![0u8; 4096];
        let address = buffer.as_mut_ptr() as usize;
        let guard = MemHandle::map_slice(&context, &mut buffer, 0).expect("map");
        let attr = guard.mem_handle().query().expect("query");
        assert_eq!(attr.address() as usize, address, "address");
        assert!(attr.length() >= 4096, "length {}", attr.length());
        assert_eq!(attr.mem_type(), crate::MemoryType::Host, "mem_type");
    }
}

/// #210: registered and device memory through the owned operations.
#[cfg(test)]
mod transfer_tests {
    use super::*;
    use crate::completion::{owned_data_param, owned_recv_data_param};
    use crate::context::{Config, Flags, ParamsBuilder};
    use crate::rma::RemoteKey;
    use crate::tag::TagInfo;
    use crate::tests::setup_default;
    use crate::worker::Worker;
    use crate::{ffi::ucp_op_attr_t, TagRecvRequest};
    use crate::{Completion, Failed, MemoryType, OwningRequest, RequestParamBuilder};

    const MEMH: u32 = ucp_op_attr_t::UCP_OP_ATTR_FIELD_MEMH as u32;
    const MEMORY_TYPE: u32 = ucp_op_attr_t::UCP_OP_ATTR_FIELD_MEMORY_TYPE as u32;

    fn transport() -> String {
        format!(
            "UCX {}, UCX_TLS={:?}",
            crate::version::get_version_string(),
            std::env::var("UCX_TLS").ok()
        )
    }

    /// Test-only progress driver for one owned completion.
    fn drive<D>(worker: &Worker, completion: Completion<D, OwningRequest<'_, D>>, what: &str) -> D {
        let mut request = match completion {
            Completion::Immediate(data) => return data,
            Completion::Pending(request) => request,
        };
        for _ in 0..1_000_000 {
            worker.progress();
            match request.try_complete() {
                Ok(Ok(data)) => return data,
                Ok(Err(failed)) => panic!("{what}: {} ({})", failed.status, transport()),
                Err(still) => request = still,
            }
        }
        panic!("{what} did not complete ({})", transport());
    }

    fn drive_recv<D>(
        worker: &Worker,
        completion: Completion<(D, TagInfo), TagRecvRequest<'_, D>>,
    ) -> (D, TagInfo) {
        let mut request = match completion {
            Completion::Immediate(done) => return done,
            Completion::Pending(request) => request,
        };
        for _ in 0..1_000_000 {
            worker.progress();
            match request.try_complete() {
                Ok(Ok(done)) => return done,
                Ok(Err(failed)) => panic!("recv: {} ({})", failed.status, transport()),
                Err(still) => request = still,
            }
        }
        panic!("recv did not complete ({})", transport());
    }

    /// Send `data` to the endpoint's own worker and receive it into `into`,
    /// with the receive posted first (so large messages take rendezvous).
    fn tag_round_trip<S: OwnedBuf, R: OwnedBufMut>(
        comms: &crate::tests::CommsContext,
        data: S,
        into: R,
        tag: u64,
    ) -> (S, R, TagInfo) {
        let param = RequestParamBuilder::new().build();
        let recv = comms
            .worker
            .tag_recv(into, tag, u64::MAX, &param)
            .expect("post recv");
        let send = comms
            .ep
            .tag_send(&comms.worker, data, tag, &param)
            .expect("post send");
        let sent = drive(&comms.worker, send, "send");
        let (received, info) = drive_recv(&comms.worker, recv);
        (sent, received, info)
    }

    fn pattern(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31) ^ seed)
            .collect()
    }

    /// Compare without Debug-printing megabytes on failure.
    fn assert_bytes(actual: &[u8], expected: &[u8], what: &str) {
        assert_eq!(actual.len(), expected.len(), "{what}: length");
        if let Some(i) = actual.iter().zip(expected).position(|(a, b)| a != b) {
            panic!(
                "{what}: first difference at byte {i}: {} != {} ({})",
                actual[i],
                expected[i],
                transport()
            );
        }
    }

    #[test]
    fn owned_data_param_passes_memory_type_and_memh() {
        let comms = setup_default();
        // A caller-supplied memory type / handle is still stripped.
        let mut param = RequestParamBuilder::new()
            .memory_type(MemoryType::Cuda)
            .build();
        param.handle.op_attr_mask |= MEMH;
        param.handle.memh = 0x1000 as _;

        let host = vec![0u8; 64];
        let raw = owned_data_param(&param, &host, &comms.worker).expect("vec");
        assert_eq!(raw.op_attr_mask & (MEMH | MEMORY_TYPE), MEMORY_TYPE);
        assert_eq!(raw.memory_type, MemoryType::Host.to_ffi());

        let registered = RegisteredBuf::new(&comms.context, vec![0u8; 64]).expect("register");
        assert_eq!(OwnedBuf::memory_type(&registered), MemoryType::Host);
        let raw = owned_recv_data_param(&param, &registered, &comms.worker).expect("registered");
        assert_eq!(raw.op_attr_mask & (MEMH | MEMORY_TYPE), MEMH | MEMORY_TYPE);
        assert_eq!(raw.memh, registered.mem_handle().as_raw());

        // An empty buffer has nothing to cover: no memory handle.
        let raw = owned_data_param(&param, &Vec::<u8>::new(), &comms.worker).expect("empty");
        assert_eq!(raw.op_attr_mask & MEMH, 0);

        // A buffer that does not override the defaults leaves both to UCX.
        struct Plain(Vec<u8>);
        // SAFETY: delegates to the Vec, which is never reallocated here.
        unsafe impl OwnedBuf for Plain {
            fn as_ptr(&self) -> *const u8 {
                self.0.as_ptr()
            }
            fn len(&self) -> usize {
                self.0.len()
            }
        }
        let raw = owned_data_param(&param, &Plain(vec![1]), &comms.worker).expect("plain");
        assert_eq!(raw.op_attr_mask & (MEMH | MEMORY_TYPE), 0);
    }

    #[test]
    fn registered_buf_rejects_empty_and_returns_buffer() {
        let comms = setup_default();
        let failed: Failed<Vec<u8>> =
            RegisteredBuf::new(&comms.context, Vec::with_capacity(8)).expect_err("empty");
        assert_eq!(failed.status, Status::INVALID_PARAM);
        assert_eq!(failed.data.capacity(), 8, "the buffer comes back");
        assert_eq!(
            AllocatedBuf::new(&comms.context, 0, MemoryType::Host).err(),
            Some(Status::INVALID_PARAM)
        );
    }

    /// #210 acceptance (host): registered buffers go through tag send/recv
    /// (eager and rendezvous) and RMA put/get with the same `Completion` /
    /// `OwningRequest` types as a `Vec<u8>`, and come back still registered.
    #[test]
    fn registered_host_buffers_tag_and_rma_round_trip() {
        let comms = setup_default();
        for len in [64usize, 8 << 20] {
            let expected = pattern(len, 0x5a);
            let send = RegisteredBuf::new(&comms.context, expected.clone()).expect("register send");
            let recv = RegisteredBuf::new(&comms.context, vec![0u8; len]).expect("register recv");
            let (send, recv, info) = tag_round_trip(&comms, send, recv, 0x210);
            assert_eq!(info.len(), len);
            assert_bytes(recv.get_ref(), &expected, "tag recv");
            assert_eq!(send.into_inner(), expected, "send buffer handed back");
        }

        let len = 1 << 20;
        let target = RegisteredBuf::new(&comms.context, vec![0u8; len]).expect("register target");
        let target_addr = target.as_ptr() as u64;
        let packed = RemoteKey::pack(&comms.context, target.mem_handle()).expect("pack rkey");
        let rkey = RemoteKey::unpack(&comms.ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().build();
        let expected = pattern(len, 0xc3);
        let source = RegisteredBuf::new(&comms.context, expected.clone()).expect("register source");
        let put = comms
            .ep
            .rma_put(&comms.worker, source, target_addr, &rkey, &param)
            .expect("post put");
        let _source: RegisteredBuf<Vec<u8>> = drive(&comms.worker, put, "put");
        drive(
            &comms.worker,
            comms.ep.flush(&comms.worker, &param).expect("flush"),
            "flush",
        );
        assert_bytes(target.get_ref(), &expected, "put landed");
        let into = RegisteredBuf::new(&comms.context, vec![0u8; len]).expect("register get");
        let get = comms
            .ep
            .rma_get(&comms.worker, into, target_addr, &rkey, &param)
            .expect("post get");
        let got = drive(&comms.worker, get, "get");
        assert_bytes(got.get_ref(), &expected, "get");
        drop(rkey);
    }

    /// A buffer whose memory handle is not usable for it: a handle of another
    /// buffer, or a memory type that disagrees with the handle.
    struct Mismatched<'a> {
        bytes: Vec<u8>,
        memh: &'a MemHandle,
        memory_type: MemoryType,
    }
    // SAFETY: the storage is the Vec's; the memory type and handle are wrong
    // on purpose, and the tests check they are rejected before UCX sees them.
    unsafe impl OwnedBuf for Mismatched<'_> {
        fn as_ptr(&self) -> *const u8 {
            self.bytes.as_ptr()
        }
        fn len(&self) -> usize {
            self.bytes.len()
        }
        fn memory_type(&self) -> MemoryType {
            self.memory_type
        }
        fn memh(&self) -> Option<&MemHandle> {
            Some(self.memh)
        }
    }
    // SAFETY: as above.
    unsafe impl OwnedBufMut for Mismatched<'_> {
        fn as_mut_ptr(&mut self) -> *mut u8 {
            self.bytes.as_mut_ptr()
        }
    }

    fn err<T>(result: Result<T, Status>) -> Status {
        match result {
            Ok(_) => panic!("must be rejected"),
            Err(status) => status,
        }
    }

    /// Every check in `MemHandle::check_covers` fails the post with
    /// `UCS_ERR_INVALID_PARAM` before anything reaches UCX.
    #[test]
    fn mismatched_memh_is_rejected_before_posting() {
        let comms = setup_default();
        let param = RequestParamBuilder::new().build();
        let other = RegisteredBuf::new(&comms.context, vec![0u8; 4096]).expect("register");
        let not_covering = || Mismatched {
            bytes: vec![1u8; 64],
            memh: other.mem_handle(),
            memory_type: MemoryType::Host,
        };
        assert_eq!(
            err(comms.ep.tag_send(&comms.worker, not_covering(), 1, &param)),
            Status::INVALID_PARAM
        );
        assert_eq!(
            err(comms.worker.tag_recv(not_covering(), 1, u64::MAX, &param)),
            Status::INVALID_PARAM
        );
        assert_eq!(
            err(comms.ep.stream_send(&comms.worker, not_covering(), &param)),
            Status::INVALID_PARAM
        );

        // The range and type checks directly, on the handle's own range: a
        // matching type passes, a wrong type or one byte too many does not.
        let attr = other.mem_handle().query().expect("query");
        assert_eq!(
            other.mem_handle().check_covers(
                &comms.worker,
                attr.address() as _,
                64,
                MemoryType::Cuda
            ),
            Err(Status::INVALID_PARAM)
        );
        assert_eq!(
            other.mem_handle().check_covers(
                &comms.worker,
                attr.address() as _,
                64,
                MemoryType::Host
            ),
            Ok(())
        );
        assert_eq!(
            other.mem_handle().check_covers(
                &comms.worker,
                attr.address() as _,
                attr.length() + 1,
                MemoryType::Unknown
            ),
            Err(Status::INVALID_PARAM),
            "one byte past the mapping"
        );

        // A handle registered on another context.
        let config = Config::read("", "").expect("config");
        let params = ParamsBuilder::new()
            .features(Flags::Tag)
            .mt_workers_shared(1)
            .build();
        let foreign_context = Context::new(&config, &params).expect("second context");
        let foreign = RegisteredBuf::new(&foreign_context, vec![0u8; 64]).expect("register");
        assert_eq!(
            err(comms.ep.tag_send(&comms.worker, foreign, 1, &param)),
            Status::INVALID_PARAM
        );
        drop(foreign_context);
        drop(other);
    }

    /// The AM header is copied with a host memcpy: a non-host header is
    /// rejected; the payload may carry a registration.
    #[test]
    fn am_send_rejects_device_header() {
        let comms = setup_default();
        let param = RequestParamBuilder::new().build();
        let holder = RegisteredBuf::new(&comms.context, vec![0u8; 64]).expect("register");
        let header = Mismatched {
            bytes: vec![0u8; 8],
            memh: holder.mem_handle(),
            memory_type: MemoryType::Cuda,
        };
        let result = comms.ep.am_send(&comms.worker, 7, header, b"", &param);
        assert_eq!(result.err(), Some(Status::INVALID_PARAM));
    }

    /// `tag_msg_recv` hands the probed message back when the buffer is
    /// rejected, so the caller can receive it with another buffer.
    #[test]
    fn tag_msg_recv_returns_message_when_buffer_rejected() {
        let comms = setup_default();
        let param = RequestParamBuilder::new().build();
        let sent = comms
            .ep
            .tag_send(&comms.worker, vec![9u8; 32], 0x77, &param)
            .expect("send");
        let _ = drive(&comms.worker, sent, "send");
        let mut message = None;
        for _ in 0..100_000 {
            comms.worker.progress();
            message = comms.worker.tag_probe(0x77, u64::MAX, true);
            if message.is_some() {
                break;
            }
        }
        let message = message.expect("probe");
        let other = RegisteredBuf::new(&comms.context, vec![0u8; 4096]).expect("register");
        let rejected = Mismatched {
            bytes: vec![0u8; 32],
            memh: other.mem_handle(),
            memory_type: MemoryType::Host,
        };
        let error = comms
            .worker
            .tag_msg_recv(rejected, message, &param)
            .err()
            .expect("rejected");
        assert_eq!(error.status, Status::INVALID_PARAM);
        let message = error.message.expect("message handed back");
        let received = comms
            .worker
            .tag_msg_recv(vec![0u8; 32], message, &param)
            .expect("receive");
        let (data, info) = drive_recv(&comms.worker, received);
        assert_eq!((data, info.len()), (vec![9u8; 32], 32));
    }

    /// Make device 0's primary CUDA context current on this thread, as any CUDA
    /// application has: UCX's CUDA allocator and transports use the calling
    /// thread's current context (without one, allocation fails with
    /// `UCS_ERR_NO_MEMORY` and transfers abort with "No such device"). The
    /// driver library is loaded at run time, so the crate needs no CUDA to
    /// build. The context is retained for the rest of the process.
    fn make_cuda_context_current() -> Result<(), String> {
        type CuInit = unsafe extern "C" fn(u32) -> i32;
        type CuDeviceGet = unsafe extern "C" fn(*mut i32, i32) -> i32;
        type CuPrimaryCtxRetain = unsafe extern "C" fn(*mut *mut libc::c_void, i32) -> i32;
        type CuCtxSetCurrent = unsafe extern "C" fn(*mut libc::c_void) -> i32;
        // SAFETY: dlopen/dlsym with NUL-terminated names; each symbol is cast
        // to the CUDA driver API signature it has (cuda.h), and the library
        // handle is never closed, so the function pointers stay valid.
        unsafe {
            let lib = libc::dlopen(c"libcuda.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
            if lib.is_null() {
                return Err("libcuda.so.1 not found".into());
            }
            let symbol = |name: &std::ffi::CStr| {
                let address = libc::dlsym(lib, name.as_ptr());
                if address.is_null() {
                    Err(format!("{name:?} not found"))
                } else {
                    Ok(address)
                }
            };
            let cu_init: CuInit = std::mem::transmute(symbol(c"cuInit")?);
            let cu_device_get: CuDeviceGet = std::mem::transmute(symbol(c"cuDeviceGet")?);
            let cu_retain: CuPrimaryCtxRetain =
                std::mem::transmute(symbol(c"cuDevicePrimaryCtxRetain")?);
            let cu_set_current: CuCtxSetCurrent = std::mem::transmute(symbol(c"cuCtxSetCurrent")?);
            let check = |what: &str, result: i32| match result {
                0 => Ok(()),
                error => Err(format!("{what} failed: CUresult {error}")),
            };
            check("cuInit", cu_init(0))?;
            let mut device = 0;
            check("cuDeviceGet", cu_device_get(&mut device, 0))?;
            let mut context = std::ptr::null_mut();
            check("cuDevicePrimaryCtxRetain", cu_retain(&mut context, device))?;
            check("cuCtxSetCurrent", cu_set_current(context))
        }
    }

    /// #210 acceptance (device): CUDA memory allocated through UCX goes through
    /// the same owned tag and RMA operations and `Completion` types as host
    /// buffers, with the memory type taken from the buffer, not from a
    /// caller-built `RequestParam`. Contents are checked by moving them back
    /// to host memory through UCX.
    ///
    /// Needs a CUDA-enabled UCX, a GPU, and a CUDA transport in `UCX_TLS`
    /// (the default, or e.g. `cuda_copy,self`; with `UCX_TLS=self` UCX has no
    /// CUDA memory domain and the allocation fails with `UCS_ERR_NO_MEMORY`),
    /// so it is ignored by default:
    /// `cargo test --lib cuda_device_buffers -- --ignored`.
    #[test]
    #[ignore = "needs a CUDA-enabled UCX and a GPU; run with --ignored"]
    fn cuda_device_buffers_tag_and_rma_round_trip() {
        make_cuda_context_current().unwrap_or_else(|e| panic!("CUDA driver: {e}"));
        let comms = setup_default();
        let alloc = |len| match AllocatedBuf::new(&comms.context, len, MemoryType::Cuda) {
            Ok(buffer) => buffer,
            Err(status) => panic!("no CUDA memory from UCX: {status} ({})", transport()),
        };
        for len in [64usize, 4 << 20] {
            let expected = pattern(len, 0x11);
            // host -> device
            let device = alloc(len);
            assert_eq!(OwnedBuf::memory_type(&device), MemoryType::Cuda);
            let (_, device, info) = tag_round_trip(&comms, expected.clone(), device, 0x2100);
            assert_eq!(info.len(), len);
            // device -> host
            let (_, host, _) = tag_round_trip(&comms, device, vec![0u8; len], 0x2101);
            assert_bytes(&host, &expected, "tag host->device->host");
        }

        // RMA: put from device memory into a registered host target, then get
        // the target back into device memory and read that out.
        let len = 1 << 20;
        let expected = pattern(len, 0x77);
        let (_, device, _) = tag_round_trip(&comms, expected.clone(), alloc(len), 0x2102);
        let target = RegisteredBuf::new(&comms.context, vec![0u8; len]).expect("register target");
        let target_addr = target.as_ptr() as u64;
        let packed = RemoteKey::pack(&comms.context, target.mem_handle()).expect("pack rkey");
        let rkey = RemoteKey::unpack(&comms.ep, &packed).expect("unpack rkey");
        let param = RequestParamBuilder::new().build();
        let put = comms
            .ep
            .rma_put(&comms.worker, device, target_addr, &rkey, &param)
            .expect("post put");
        let _device: AllocatedBuf = drive(&comms.worker, put, "put");
        drive(
            &comms.worker,
            comms.ep.flush(&comms.worker, &param).expect("flush"),
            "flush",
        );
        assert_bytes(target.get_ref(), &expected, "put device->host");
        let get = comms
            .ep
            .rma_get(&comms.worker, alloc(len), target_addr, &rkey, &param)
            .expect("post get");
        let got = drive(&comms.worker, get, "get");
        drop(rkey);
        let (_, host, _) = tag_round_trip(&comms, got, vec![0u8; len], 0x2103);
        assert_bytes(&host, &expected, "get host->device");
    }
}
