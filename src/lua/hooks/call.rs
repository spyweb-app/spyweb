use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use mlua::{Lua, Table, Value};
use smol::lock::{Mutex, futures::Lock};

pub(crate) struct HookCall<'a> {
    lua: &'a Mutex<Lua>,
    ctx: Table,
    lock: Option<Pin<Box<Lock<'a, Lua>>>>,
    inner: Option<Pin<Box<dyn Future<Output = mlua::Result<Value>> + Send>>>,
}

impl<'a> HookCall<'a> {
    // Wrap an already-created hook future; the caller did the locked prep.
    pub(crate) fn new<F>(lua: &'a Mutex<Lua>, ctx: Table, inner: F) -> Self
    where
        F: Future<Output = mlua::Result<Value>> + Send + 'static,
    {
        Self {
            lua,
            ctx,
            lock: None,
            inner: Some(Box::pin(inner)),
        }
    }
}

impl Future for HookCall<'_> {
    type Output = mlua::Result<Value>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // stashed across polls so async-lock registers the waker exactly
        // once (no spin); cleared the moment we own the guard.
        let guard = {
            let lock = this.lock.get_or_insert_with(|| Box::pin(this.lua.lock()));
            match lock.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(guard) => {
                    this.lock = None;
                    guard
                }
            }
        };

        // invariant write point #2: re-set active_ctx on every resume —
        // another worker's poll overwrote the slot while we awaited.
        if let Err(e) = guard.set_named_registry_value("active_ctx", this.ctx.clone()) {
            this.inner = None;
            return Poll::Ready(Err(e));
        }

        let Some(inner) = this.inner.as_mut() else {
            return Poll::Ready(Err(mlua::Error::runtime(
                "HookCall polled after completion",
            )));
        };
        match inner.as_mut().poll(cx) {
            Poll::Pending => {
                drop(guard);
                Poll::Pending
            }
            Poll::Ready(r) => {
                this.inner = None;
                drop(guard);
                Poll::Ready(r)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Function;
    use std::time::{Duration, Instant};

    #[allow(dead_code)]
    fn gate_a_bounds() {
        fn assert_send<T: Send>() {}
        assert_send::<HookCall<'static>>();
    }

    fn lua_with_sleep() -> Lua {
        let lua = Lua::new();
        lua.globals()
            .set(
                "sleep",
                lua.create_async_function(|_, ms: u64| async move {
                    smol::Timer::after(Duration::from_millis(ms)).await;
                    Ok(())
                })
                .unwrap(),
            )
            .unwrap();
        lua
    }

    async fn prepare<'a>(
        m: &'a Mutex<Lua>,
        chunk: &str,
        worker_id: u32,
    ) -> mlua::Result<HookCall<'a>> {
        let lua = m.lock().await;
        lua.load(chunk)
            .set_name(format!("=gate_b_{worker_id}"))
            .exec()?;
        let func: Function = lua.globals().get("hook")?;
        let ctx = lua.create_table()?;
        ctx.set("worker_id", worker_id)?;
        lua.set_named_registry_value("active_ctx", ctx.clone())?;
        let inner = func.call_async::<Value>(());
        drop(lua);
        Ok(HookCall::new(m, ctx, inner))
    }

    // two 200 ms hook calls on one VM must overlap (<350 ms), 5 s
    // watchdog converts a deadlock into a failure.
    #[test]
    fn gate_b_two_hook_calls_interleave() {
        let m = Mutex::new(lua_with_sleep());
        let start = Instant::now();

        smol::block_on(async {
            let work = async {
                let h1 = prepare(&m, "function hook() sleep(200); return 1 end", 1)
                    .await
                    .unwrap();
                let h2 = prepare(&m, "function hook() sleep(200); return 2 end", 2)
                    .await
                    .unwrap();
                let (r1, r2) = smol::future::zip(h1, h2).await;
                assert_eq!(r1.unwrap().as_integer(), Some(1));
                assert_eq!(r2.unwrap().as_integer(), Some(2));
            };
            smol::future::or(work, async {
                smol::Timer::after(Duration::from_secs(5)).await;
                panic!("gate_b: HookCall deadlock or hang (5s watchdog)");
            })
            .await;
        });

        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(350),
            "gate_b: serial behavior — expected <350 ms (interleaved ~200 ms), took {elapsed:?}"
        );
    }
}
