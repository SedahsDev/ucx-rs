#[cfg(test)]
mod asserts {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    // These wrappers are tied to the UCX object/progress context that owns them.
    assert_not_impl_any!(crate::worker::Worker: Send, Sync);
    assert_not_impl_any!(crate::ep::Ep: Send, Sync);
    assert_not_impl_any!(crate::Request: Send, Sync);
    assert_not_impl_any!(crate::rma::RemoteKey: Send, Sync);
    assert_not_impl_any!(crate::memh::MemHandle: Send, Sync);

    // Borrowed completion/data guards must remain on their owning context.
    assert_not_impl_any!(crate::rma::FetchAmoRequest<'static, 'static, u64>: Send);
    assert_not_impl_any!(crate::stream::StreamData<'static>: Send);
    assert_not_impl_any!(crate::memh::MemHandleGuard<'static>: Send);
    assert_not_impl_any!(crate::worker::WorkerAddress<'static>: Send);

    // Plain result/status values and snapshots are thread-safe value types.
    assert_impl_all!(crate::RequestState: Send, Sync);
    assert_impl_all!(crate::RequestAttr: Send, Sync);
    assert_impl_all!(crate::ucs_status_t: Send, Sync);
    assert_impl_all!(Result<(), crate::ucs_status_t>: Send, Sync);

    // Parameter structs embed UCX raw pointers and are therefore not Send on
    // this platform; no positive assertion is made for them.
}
