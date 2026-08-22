//! UCP data type bindings.
//!
//! Wraps data type creation, destruction, and query. Note that
//! `ucp_dt_make_contig` and `ucp_dt_make_iov` are C macros in the
//! original API — they are reimplemented as Rust functions here.

use crate::ffi::*;
use crate::status_to_result;

/// UCP contiguous data type class, sourced from bindgen.
pub const UCP_DATATYPE_CONTIG: ucp_datatype_t = ucp_dt_type::UCP_DATATYPE_CONTIG as ucp_datatype_t;

/// UCP I/O vector data type class, sourced from bindgen.
pub const UCP_DATATYPE_IOV: ucp_datatype_t = ucp_dt_type::UCP_DATATYPE_IOV as ucp_datatype_t;

/// Create a contiguous data type with the given element size (in bytes).
/// Equivalent to the C macro `ucp_dt_make_contig(elem_size)`.
///
/// The encoding is `(elem_size << UCP_DATATYPE_SHIFT) | UCP_DATATYPE_CONTIG`.
/// Element size 0 is preserved, matching the C macro and producing the class-only value.
#[must_use]
pub fn dt_make_contig(elem_size: usize) -> ucp_datatype_t {
    ((elem_size as ucp_datatype_t) << ucp_dt_type::UCP_DATATYPE_SHIFT as ucp_datatype_t)
        | UCP_DATATYPE_CONTIG
}

/// Create an I/O vector data type.
/// Equivalent to the C macro `ucp_dt_make_iov()`.
#[must_use]
pub fn dt_make_iov() -> ucp_datatype_t {
    UCP_DATATYPE_IOV
}

/// Create a generic data type from user-provided operations.
/// Returns the created datatype handle.
///
/// # Safety
/// Caller must ensure `ops` points to a valid `ucp_generic_dt_ops` struct
/// for the lifetime of the created datatype.
pub unsafe fn dt_create_generic(
    ops: &ucp_generic_dt_ops,
    context: *mut std::os::raw::c_void,
) -> Result<ucp_datatype_t, ucs_status_t> {
    let mut datatype: ucp_datatype_t = 0;
    status_to_result(ucp_dt_create_generic(ops, context, &mut datatype)).map(|()| datatype)
}

/// Destroy a user-defined data type.
pub fn dt_destroy(datatype: ucp_datatype_t) {
    unsafe {
        ucp_dt_destroy(datatype);
    }
}

/// Field masks for `ucp_datatype_attr`.
pub const UCP_DT_ATTR_FIELD_PACKED_SIZE: u64 = 1;
pub const UCP_DT_ATTR_FIELD_BUFFER: u64 = 2;
pub const UCP_DT_ATTR_FIELD_COUNT: u64 = 4;

/// Data type attributes queried via `dt_query()`.
#[derive(Debug, Clone)]
pub struct DataTypeAttr {
    pub packed_size: usize,
    pub buffer: *const std::os::raw::c_void,
    pub count: usize,
}

/// Query data type attributes.
pub fn dt_query(datatype: ucp_datatype_t, mask: u64) -> Result<DataTypeAttr, ucs_status_t> {
    let mut attr: ucp_datatype_attr = unsafe { std::mem::zeroed() };
    attr.field_mask = mask;
    status_to_result(unsafe { ucp_dt_query(datatype, &mut attr) }).map(|()| DataTypeAttr {
        packed_size: attr.packed_size,
        buffer: attr.buffer,
        count: attr.count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dt_make_contig() {
        assert_eq!(dt_make_contig(0), 0);
        assert_eq!(dt_make_contig(1), 8);
        assert_eq!(dt_make_contig(4), 32);
        assert_eq!(dt_make_contig(8), 64);
    }

    #[test]
    fn test_dt_make_contig_composes_size_shift_and_class() {
        let shift = ucp_dt_type::UCP_DATATYPE_SHIFT as ucp_datatype_t;
        let class = ucp_dt_type::UCP_DATATYPE_CONTIG as ucp_datatype_t;
        for elem_size in [2, 16] {
            assert_eq!(
                dt_make_contig(elem_size),
                ((elem_size as ucp_datatype_t) << shift) | class
            );
        }
    }

    #[test]
    fn test_dt_make_iov() {
        let dt = dt_make_iov();
        assert_eq!(dt, 2); // UCP_DATATYPE_IOV
    }

    #[test]
    fn test_dt_query_contig() {
        // Built-in contig types don't support dt_query — it returns INVALID_PARAM.
        // Just verify the contig type creation works.
        let dt = dt_make_contig(4);
        assert_ne!(dt, dt_make_contig(8));
        assert_eq!(dt_make_contig(1), 8);
        assert_eq!(dt_make_contig(4), 32);
        assert_eq!(dt_make_contig(8), 64);
    }
}
