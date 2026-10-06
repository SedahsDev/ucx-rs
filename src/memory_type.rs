//! Memory type enum for memory registration and operations.
//!
//! This wraps the `ucs_memory_type_t` FFI type and provides a safe,
//! idiomatic Rust interface.

/// Memory type for memory registration and operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemoryType {
    /// Host memory.
    Host,
    /// CUDA device memory.
    Cuda,
    /// CUDA managed memory.
    CudaManaged,
    /// ROCm device memory.
    Rocm,
    /// ROCm managed memory.
    RocmManaged,
    /// RDMA memory.
    Rdma,
    /// Intel oneAPI ZE host memory.
    ZeHost,
    /// Intel oneAPI ZE device memory.
    ZeDevice,
    /// Intel oneAPI ZE managed memory.
    ZeManaged,
    /// A value UCX returned that this build's enum does not name; UCX may
    /// report unknown or new memory types at runtime.
    Unknown,
}

impl MemoryType {
    /// Convert to the raw FFI memory type. `Unknown` maps to the last known
    /// variant so the conversion stays total.
    pub(crate) const fn to_ffi(self) -> crate::ffi::ucs_memory_type_t {
        match self {
            MemoryType::Host => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_HOST,
            MemoryType::Cuda => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_CUDA,
            MemoryType::CudaManaged => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_CUDA_MANAGED,
            MemoryType::Rocm => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ROCM,
            MemoryType::RocmManaged => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ROCM_MANAGED,
            MemoryType::Rdma => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_RDMA,
            MemoryType::ZeHost => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_HOST,
            MemoryType::ZeDevice => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_DEVICE,
            MemoryType::ZeManaged | MemoryType::Unknown => {
                crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_MANAGED
            }
        }
    }

    /// Convert from the raw FFI memory type. Unknown values map to
    /// [`MemoryType::Unknown`].
    pub(crate) fn from_ffi(raw: crate::ffi::ucs_memory_type_t) -> Self {
        match raw {
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_HOST => MemoryType::Host,
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_CUDA => MemoryType::Cuda,
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_CUDA_MANAGED => MemoryType::CudaManaged,
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ROCM => MemoryType::Rocm,
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ROCM_MANAGED => MemoryType::RocmManaged,
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_RDMA => MemoryType::Rdma,
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_HOST => MemoryType::ZeHost,
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_DEVICE => MemoryType::ZeDevice,
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_MANAGED => MemoryType::ZeManaged,
            _ => MemoryType::Unknown,
        }
    }
}
