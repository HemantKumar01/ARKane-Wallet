//! Inspired by
//! https://github.com/MetaMask/bdk-wasm/blob/c8c1811a4375be1882c2dfe9a3994444e7a58a85/src/utils/future.rs#L6.

use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

// Wrap a future that is not `Send` and make it `Send`.
pub struct SendWrapper<F>(pub F);

unsafe impl<F> Send for SendWrapper<F> {}

impl<F> Future for SendWrapper<F>
where
    F: Future,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: Since we're in a single-threaded WASM environment, this is safe.
        unsafe {
            let this = self.get_unchecked_mut();
            Pin::new_unchecked(&mut this.0).poll(cx)
        }
    }
}
