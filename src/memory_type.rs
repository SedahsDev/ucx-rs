//! Memory type enum for memory registration and operations.
//!
//! This wraps the `ucs_memory_type_t` FFI type and provides a safe,
//! idiomatic Rust interface.

/// Memory type for memory registration and operations.
///
/// This enum is `#[non_exhaustive]`: variants may be added when the crate supports newer UCX
/// versions, so a `match` on it outside this crate needs a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
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
    /// Unknown memory type. Passed to a setter such as `RequestParamBuilder::memory_type` or
    /// `memh::MemMapParamsBuilder::memory_type`, it asks UCX to detect the memory type of the
    /// buffer itself. Also reported when UCX returns its own "unknown" memory type.
    Unknown,
}

impl MemoryType {
    /// Convert to the raw FFI memory type. `Unknown` maps to UCX's unknown memory type, which
    /// asks UCX to detect the memory type.
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
            MemoryType::ZeManaged => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_ZE_MANAGED,
            MemoryType::Unknown => crate::ffi::ucs_memory_type_t::UCS_MEMORY_TYPE_UNKNOWN,
        }
    }

    /// Convert from the raw FFI memory type. The bindgen enum is a real Rust enum, so the only
    /// other value it can hold is UCX's unknown memory type (equal to its end-of-enum
    /// sentinel), which maps to `Unknown`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::ucs_memory_type_t;

    #[test]
    fn memory_type_unknown_maps_to_ucx_unknown() {
        assert_eq!(
            MemoryType::Unknown.to_ffi(),
            ucs_memory_type_t::UCS_MEMORY_TYPE_UNKNOWN
        );
        assert_eq!(
            MemoryType::from_ffi(ucs_memory_type_t::UCS_MEMORY_TYPE_UNKNOWN),
            MemoryType::Unknown
        );
    }

    #[test]
    fn memory_type_known_variants_round_trip() {
        for memory_type in [
            MemoryType::Host,
            MemoryType::Cuda,
            MemoryType::CudaManaged,
            MemoryType::Rocm,
            MemoryType::RocmManaged,
            MemoryType::Rdma,
            MemoryType::ZeHost,
            MemoryType::ZeDevice,
            MemoryType::ZeManaged,
        ] {
            assert_eq!(MemoryType::from_ffi(memory_type.to_ffi()), memory_type);
        }
    }
}
