//! The interface for MCP clients: Scaleform movies on a headless GPU, on their own thread
//! (Ruffle players are not `Send`). Tools send closures to the thread and await the result.

use std::io::Cursor;
use std::sync::Arc;
use std::sync::mpsc;

use thedas_ui::protocol::GameMessage;
use thedas_ui::stage::Stage;
use thedas_ui::{Gpu, Rgba, UiValue};
use thedas_vfs::Vfs;

/// Everything the UI thread owns.
#[derive(Default)]
pub struct UiWorld {
    gpu: Option<Gpu>,
    pub stage: Option<Stage>,
    /// Messages for the engine since the last read.
    pub messages: Vec<GameMessage>,
}

impl UiWorld {
    fn gpu(&mut self) -> Result<Gpu, String> {
        if self.gpu.is_none() {
            let ctx = thedas_render::GpuContext::headless().map_err(|e| e.to_string())?;
            self.gpu = Some(Gpu {
                instance: ctx.instance,
                adapter: ctx.adapter,
                device: ctx.device,
                queue: ctx.queue,
            });
        }
        self.gpu.clone().ok_or_else(|| "no GPU".to_string())
    }

    /// The stage, created for `vfs` at `size` if there is none (or the size changed).
    pub fn stage(&mut self, vfs: &Arc<Vfs>, size: (u32, u32)) -> Result<&mut Stage, String> {
        if self.stage.as_ref().map(|s| s.size()) != Some(size) {
            let gpu = self.gpu()?;
            self.stage = Some(Stage::new(gpu, vfs, size));
        }
        self.stage.as_mut().ok_or_else(|| "no stage".to_string())
    }

    /// Runs `frames` movie frames at 30 fps with the given engine values bound.
    pub fn step(&mut self, frames: u32, values: &[(String, UiValue)]) {
        let Some(stage) = self.stage.as_mut() else {
            return;
        };
        for _ in 0..frames.max(1) {
            stage.bridge.set_values(values.iter().cloned());
            self.messages.extend(stage.tick(1000.0 / 30.0));
        }
    }

    /// The movies composited over black, as PNG bytes.
    pub fn screenshot(&mut self) -> Result<Vec<u8>, String> {
        let stage = self.stage.as_mut().ok_or("no movies open; call ui_open")?;
        let (w, h) = stage.size();
        let mut frame = Rgba {
            width: w,
            height: h,
            pixels: [0, 0, 0, 255].repeat(w as usize * h as usize),
        };
        stage.capture_over(&mut frame).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(Cursor::new(&mut out), w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().map_err(|e| e.to_string())?;
            wr.write_image_data(&frame.pixels)
                .map_err(|e| e.to_string())?;
        }
        Ok(out)
    }
}

type Job = Box<dyn FnOnce(&mut UiWorld) + Send>;

/// Handle to the UI thread.
#[derive(Clone)]
pub struct UiThread {
    tx: mpsc::Sender<Job>,
}

impl UiThread {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("thedas-ui".into())
            .spawn(move || {
                let mut world = UiWorld::default();
                while let Ok(job) = rx.recv() {
                    job(&mut world);
                }
            })
            .ok();
        Self { tx }
    }

    /// Runs `f` on the UI thread and returns its result.
    pub async fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut UiWorld) -> R + Send + 'static,
    ) -> Result<R, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Box::new(move |w: &mut UiWorld| {
                let _ = tx.send(f(w));
            }))
            .map_err(|_| "the UI thread is gone".to_string())?;
        rx.await
            .map_err(|_| "the UI thread dropped the job".to_string())
    }
}
