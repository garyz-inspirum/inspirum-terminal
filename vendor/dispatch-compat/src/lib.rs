//! Minimal compatibility surface for objc2-foundation 0.2.x.
//!
//! The application only needs `Queue::main().exec_sync(...)` through
//! `objc2_foundation::run_on_main`. Keeping this implementation local removes
//! the obsolete third-party `dispatch 0.2.0` package from the release graph
//! while preserving the required libdispatch behavior.

#[cfg(any(target_os = "macos", target_os = "ios"))]
use std::ffi::c_void;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use super::c_void;

    #[repr(C)]
    struct DispatchObject {
        _private: [u8; 0],
    }

    type DispatchQueue = *mut DispatchObject;
    type DispatchFunction = extern "C" fn(*mut c_void);

    #[link(name = "System", kind = "dylib")]
    extern "C" {
        static _dispatch_main_q: DispatchObject;
        fn dispatch_retain(object: *mut DispatchObject);
        fn dispatch_release(object: *mut DispatchObject);
        fn dispatch_sync_f(
            queue: DispatchQueue,
            context: *mut c_void,
            work: DispatchFunction,
        );
    }

    /// A retained Grand Central Dispatch queue.
    pub struct Queue {
        raw: DispatchQueue,
    }

    impl Queue {
        /// Return the process main dispatch queue.
        pub fn main() -> Self {
            let raw = unsafe { std::ptr::addr_of!(_dispatch_main_q).cast_mut() };
            unsafe {
                dispatch_retain(raw);
            }
            Self { raw }
        }

        /// Execute a closure synchronously on this queue and return its result.
        pub fn exec_sync<T, F>(&self, work: F) -> T
        where
            F: Send + FnOnce() -> T,
            T: Send,
        {
            struct Context<F, T> {
                work: Option<F>,
                result: Option<T>,
            }

            extern "C" fn invoke<F, T>(context: *mut c_void)
            where
                F: Send + FnOnce() -> T,
                T: Send,
            {
                let context = unsafe { &mut *context.cast::<Context<F, T>>() };
                let work = context.work.take().expect("dispatch work already consumed");
                context.result = Some(work());
            }

            let mut context = Context {
                work: Some(work),
                result: None,
            };
            unsafe {
                dispatch_sync_f(
                    self.raw,
                    std::ptr::addr_of_mut!(context).cast::<c_void>(),
                    invoke::<F, T>,
                );
            }
            context
                .result
                .expect("dispatch_sync_f returned without executing work")
        }
    }

    impl Drop for Queue {
        fn drop(&mut self) {
            unsafe {
                dispatch_release(self.raw);
            }
        }
    }

    unsafe impl Send for Queue {}
    unsafe impl Sync for Queue {}
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
mod imp {
    /// Non-Apple fallback used only so metadata/tests can resolve the patched
    /// package on development hosts. The production dependency is Apple-only.
    #[derive(Debug, Default)]
    pub struct Queue;

    impl Queue {
        pub fn main() -> Self {
            Self
        }

        pub fn exec_sync<T, F>(&self, work: F) -> T
        where
            F: Send + FnOnce() -> T,
            T: Send,
        {
            work()
        }
    }
}

pub use imp::Queue;

#[cfg(test)]
mod tests {
    use super::Queue;

    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    #[test]
    fn fallback_executes_once_and_returns_result() {
        assert_eq!(Queue::main().exec_sync(|| 42), 42);
    }
}
