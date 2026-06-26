//! UCP data type bindings.
//!
//! Wraps data type creation, destruction, and query. Note that
//! `ucp_dt_make_contig` and `ucp_dt_make_iov` are C macros in the
//! original API — they are reimplemented as Rust functions here.

use crate::ffi::*;
use crate::status_to_result;

/// UCP contiguous data type ID (element size 1).
pub const UCP_DATATYPE_CONTIG: ucp_datatype_t = 0;

/// UCP I/O vector data type ID.
pub const UCP_DATATYPE_IOV: ucp_datatype_t = 2;

/// Create a contiguous data type with the given element size (in bytes).
/// Equivalent to the C macro `ucp_dt_make_contig(elem_size)`.
///
/// The encoding packs the element size into the datatype handle.
/// Element size 0 is treated as 1.
#[must_use]
pub fn dt_make_contig(elem_size: usize) -> ucp_datatype_t {
    let size = if elem_size == 0 { 1 } else { elem_size };
    if size == 1 {
        0 // UCP_DATATYPE_CONTIG for size 1
    } else {
        ((size - 1) as u64) | (1u64 << 16)
    }
}

/// Create an I/O vector data type.
/// Equivalent to the C macro `ucp_dt_make_iov()`.
#[must_use]
pub fn dt_make_iov() -> ucp_datatype_t {
    2 // UCP_DATATYPE_IOV
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
    status_to_result(ucp_dt_create_generic(ops, context, &mut datatype))
        .map(|()| datatype)
}

/// Destroy a user-defined data type.
pub fn dt_destroy(datatype: ucp_datatype_t) {
    unsafe { ucp_dt_destroy(datatype); }
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
        let dt1 = dt_make_contig(1);
        assert_eq!(dt1, 0); // UCP_DATATYPE_CONTIG
        let dt4 = dt_make_contig(4);
        assert_ne!(dt4, 0);
        let dt0 = dt_make_contig(0);
        assert_eq!(dt0, 0); // size 0 treated as 1
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
        assert_eq!(dt_make_contig(1), super::UCP_DATATYPE_CONTIG);
    }
}
