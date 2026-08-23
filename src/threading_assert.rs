#[cfg(test)]
mod asserts {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    // Sessions/context: Context owns the session handle and UCX permits context-level
    // operations across workers on different threads; workers remain thread-bound.
    assert_impl_all!(crate::context::Context: Send, Sync);

    // These wrappers are tied to the UCX object/progress context that owns them.
    assert_not_impl_any!(crate::worker::Worker: Send, Sync);
    assert_not_impl_any!(crate::ep::Ep: Send, Sync);
    assert_not_impl_any!(crate::Request: Send, Sync);
    assert_not_impl_any!(crate::rma::RemoteKey: Send, Sync);
    assert_not_impl_any!(crate::memh::MemHandle: Send, Sync);

    // Borrowed completion/data guards must remain on their owning context.
    assert_not_impl_any!(crate::rma::FetchAmoRequest<'static, 'static, u64>: Send, Sync);
    assert_not_impl_any!(crate::stream::StreamData<'static>: Send, Sync);
    assert_not_impl_any!(crate::memh::MemHandleGuard<'static>: Send, Sync);
    assert_not_impl_any!(crate::worker::WorkerAddress<'static>: Send, Sync);

    // Plain result/status values and snapshots are thread-safe value types.
    assert_impl_all!(crate::config::ContextAttr: Send, Sync);
    assert_impl_all!(crate::version::LibAttr: Send, Sync);
    // DataTypeAttr contains a raw buffer pointer, so it intentionally remains
    // absent from the positive matrix rather than asserting unsafe threadability.
    assert_impl_all!(crate::RequestState: Send, Sync);
    assert_impl_all!(crate::RequestAttr: Send, Sync);
    assert_impl_all!(crate::ucs_status_t: Send, Sync);
    assert_impl_all!(Result<(), crate::ucs_status_t>: Send, Sync);

    // Parameter structs embed UCX raw pointers and are therefore not Send on
    // this platform; no positive assertion is made for them.
}
