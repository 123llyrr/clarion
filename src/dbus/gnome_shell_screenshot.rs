use std::collections::HashMap;
use std::path::PathBuf;

use clarion_ipc::PickedColor;
use zbus::fdo::{self, RequestNameFlags};
use zbus::zvariant::OwnedValue;
use zbus::{interface, zvariant};

use super::Start;

pub struct Screenshot {
    to_clarion: calloop::channel::Sender<ScreenshotToClarion>,
}

pub enum ScreenshotToClarion {
    TakeScreenshot {
        include_cursor: bool,
        reply: async_channel::Sender<ClarionToScreenshot>,
    },
    PickColor(async_channel::Sender<Option<PickedColor>>),
}

pub enum ClarionToScreenshot {
    ScreenshotResult(Option<PathBuf>),
}

#[interface(name = "org.gnome.Shell.Screenshot")]
impl Screenshot {
    async fn screenshot(
        &self,
        include_cursor: bool,
        _flash: bool,
        _filename: PathBuf,
    ) -> fdo::Result<(bool, PathBuf)> {
        let (reply, result) = async_channel::bounded(1);
        if let Err(err) = self.to_clarion.send(ScreenshotToClarion::TakeScreenshot {
            include_cursor,
            reply,
        }) {
            warn!("error sending message to clarion: {err:?}");
            return Err(fdo::Error::Failed("internal error".to_owned()));
        }

        let filename = match result.recv().await {
            Ok(ClarionToScreenshot::ScreenshotResult(Some(filename))) => filename,
            Ok(ClarionToScreenshot::ScreenshotResult(None)) => {
                return Err(fdo::Error::Failed("internal error".to_owned()));
            }
            Err(err) => {
                warn!("error receiving message from clarion: {err:?}");
                return Err(fdo::Error::Failed("internal error".to_owned()));
            }
        };

        Ok((true, filename))
    }

    async fn pick_color(&self) -> fdo::Result<HashMap<String, OwnedValue>> {
        let (tx, rx) = async_channel::bounded(1);
        if let Err(err) = self.to_clarion.send(ScreenshotToClarion::PickColor(tx)) {
            warn!("error sending pick color message to clarion: {err:?}");
            return Err(fdo::Error::Failed("internal error".to_owned()));
        }

        let color = match rx.recv().await {
            Ok(Some(color)) => color,
            Ok(None) => {
                return Err(fdo::Error::Failed("no color picked".to_owned()));
            }
            Err(err) => {
                warn!("error receiving message from clarion: {err:?}");
                return Err(fdo::Error::Failed("internal error".to_owned()));
            }
        };

        let mut result = HashMap::new();
        let [r, g, b] = color.rgb;
        result.insert(
            "color".to_string(),
            zvariant::OwnedValue::try_from(zvariant::Value::from((r, g, b))).unwrap(),
        );

        Ok(result)
    }
}

impl Screenshot {
    pub fn new(to_clarion: calloop::channel::Sender<ScreenshotToClarion>) -> Self {
        Self { to_clarion }
    }
}

impl Start for Screenshot {
    fn start(self) -> anyhow::Result<zbus::blocking::Connection> {
        let conn = zbus::blocking::Connection::session()?;
        let flags = RequestNameFlags::AllowReplacement
            | RequestNameFlags::ReplaceExisting
            | RequestNameFlags::DoNotQueue;

        conn.object_server()
            .at("/org/gnome/Shell/Screenshot", self)?;
        conn.request_name_with_flags("org.gnome.Shell.Screenshot", flags)?;

        Ok(conn)
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use std::time::{Duration, Instant};

    use super::*;

    fn request(rx: &calloop::channel::Channel<ScreenshotToClarion>) -> ScreenshotToClarion {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(request) = rx.try_recv() {
                return request;
            }
            assert!(
                Instant::now() < deadline,
                "screenshot did not send its request"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn concurrent_requests_receive_their_own_out_of_order_results() {
        let (tx, rx) = calloop::channel::channel();
        let screenshot = Arc::new(Screenshot::new(tx));
        let worker = std::thread::spawn(move || {
            async_io::block_on(futures_util::future::join(
                screenshot.screenshot(false, false, PathBuf::new()),
                screenshot.screenshot(true, false, PathBuf::new()),
            ))
        });
        let requests = [request(&rx), request(&rx)];
        for request in requests.into_iter().rev() {
            let ScreenshotToClarion::TakeScreenshot {
                include_cursor,
                reply,
            } = request
            else {
                panic!();
            };
            let path = PathBuf::from(if include_cursor {
                "pointer.png"
            } else {
                "screen.png"
            });
            reply
                .try_send(ClarionToScreenshot::ScreenshotResult(Some(path)))
                .unwrap();
        }
        let (first, second) = worker.join().unwrap();
        assert_eq!(first.unwrap(), (true, PathBuf::from("screen.png")));
        assert_eq!(second.unwrap(), (true, PathBuf::from("pointer.png")));
    }

    #[test]
    fn cancelled_request_cannot_supply_a_later_requests_result() {
        let (tx, rx) = calloop::channel::channel();
        let screenshot = Arc::new(Screenshot::new(tx));
        let mut first = Box::pin(screenshot.screenshot(false, false, PathBuf::new()));
        let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
        assert!(matches!(first.as_mut().poll(&mut cx), Poll::Pending));
        let ScreenshotToClarion::TakeScreenshot { reply: stale, .. } = request(&rx) else {
            panic!();
        };
        drop(first);
        let worker = std::thread::spawn(move || {
            async_io::block_on(screenshot.screenshot(false, false, PathBuf::new()))
        });
        let ScreenshotToClarion::TakeScreenshot { reply: current, .. } = request(&rx) else {
            panic!();
        };
        assert!(stale
            .try_send(ClarionToScreenshot::ScreenshotResult(Some(PathBuf::from(
                "stale.png"
            ))))
            .is_err());
        current
            .try_send(ClarionToScreenshot::ScreenshotResult(Some(PathBuf::from(
                "current.png",
            ))))
            .unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap(),
            (true, PathBuf::from("current.png"))
        );
    }
}
