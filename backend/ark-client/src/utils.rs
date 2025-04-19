use futures::Future;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) fn spawn<F>(future: F)
where
    F: Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(future);
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) fn spawn<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(future);
}

pub(crate) async fn sleep(duration: std::time::Duration) {
    #[cfg(target_arch = "wasm32")]
    {
        gloo_timers::future::sleep(duration).await
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::sleep(duration).await;
    }
}
