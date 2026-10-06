//! Memory type enum for memory registration and operations.
//!
//! This wraps the `ucs_memory_type_t` FFI type and provides a safe,
//! idiomatic Rust interface.

use crate::status::Status;

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
}

impl From<MemoryType> for crate::ffi::ucs_memory_type_t {
    fn from(mt: MemoryType) -> Self {
        match mt {
            MemoryType::Host => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_HOST,
            MemoryType::Cuda => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_CUDA,
            MemoryType::CudaManaged => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_CUDA_MANAGED,
            MemoryType::Rocm => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ROCM,
            MemoryType::RocmManaged => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ROCM_MANAGED,
            MemoryType::Rdma => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_RDMA,
            MemoryType::ZeHost => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_HOST,
            MemoryType::ZeDevice => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_DEVICE,
            MemoryType::ZeManaged => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_MANAGED,
        }
    }
}

impl TryFrom<crate::ffi::ucs_memory_type_t> for MemoryType {
    type Error = crate::Status;

    fn try_from(mt: crate::ffi::ucs_memory_type_t) -> Result<Self, Self::Error> {
        match mt {
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_HOST => Ok(MemoryType::Host),
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_CUDA => Ok(MemoryType::Cuda),
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_CUDA_MANAGED => Ok(MemoryType::CudaManaged),
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ROCM => Ok(MemoryType::Rocm),
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ROCM_MANAGED => Ok(MemoryType::RocmManaged),
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_RDMA => Ok(MemoryType::Rdma),
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_HOST => Ok(MemoryType::ZeHost),
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_DEVICE => Ok(MemoryType::ZeDevice),
            crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_MANAGED => Ok(MemoryType::ZeManaged),
            _ => Err(Status::from_raw(
                crate::ffi::ucs_status_t::UCS_ERR_INVALID_PARAM,
            )),
        }
    }
}
