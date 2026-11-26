use libpulse_binding::{
    callbacks::ListResult,
    context::{Context, FlagSet, State},
    error::PAErr,
    mainloop::standard::Mainloop,
    operation::{Operation, State as OperationState},
    time::MicroSeconds,
    volume::ChannelVolumes,
};
use std::{
    cell::{Cell, RefCell},
    io,
    rc::Rc,
    time::Instant,
};

#[derive(Clone, Copy, Debug)]
pub struct Device {
    pub index: u32,
    pub channels: ChannelVolumes,
    pub muted: bool,
}

pub struct Pulse {
    context: Context,
    mainloop: Mainloop,
    pub deadline: Instant,
}

fn connection_error(error: PAErr) -> io::Error {
    io::Error::other(
        error
            .to_string()
            .unwrap_or_else(|| "Audio connection failed".into()),
    )
}

impl Pulse {
    pub fn connect(name: &str, deadline: Instant) -> io::Result<Self> {
        let mainloop =
            Mainloop::new().ok_or_else(|| io::Error::other("Audio mainloop unavailable"))?;
        let context = Context::new(&mainloop, name)
            .ok_or_else(|| io::Error::other("Audio context unavailable"))?;
        let mut pulse = Self {
            context,
            mainloop,
            deadline,
        };
        pulse
            .context
            .connect(None, FlagSet::NOAUTOSPAWN, None)
            .map_err(connection_error)?;
        while pulse.context.get_state() != State::Ready {
            pulse.pump()?;
        }
        Ok(pulse)
    }

    pub fn alive(&mut self) -> bool {
        self.mainloop.iterate(false).is_success() && self.context.get_state() == State::Ready
    }

    fn pump(&mut self) -> io::Result<()> {
        if matches!(self.context.get_state(), State::Failed | State::Terminated) {
            return Err(io::Error::other("Audio service unavailable"));
        }
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::other("Audio request timed out"));
        }
        self.mainloop
            .prepare(Some(MicroSeconds(remaining.as_micros() as u64)))
            .and_then(|()| self.mainloop.poll())
            .and_then(|_| self.mainloop.dispatch())
            .map_err(connection_error)?;
        Ok(())
    }

    fn wait<T: ?Sized>(&mut self, mut operation: Operation<T>) -> io::Result<()> {
        while operation.get_state() == OperationState::Running {
            if let Err(error) = self.pump() {
                operation.cancel();
                return Err(error);
            }
        }
        if operation.get_state() != OperationState::Done {
            return Err(io::Error::other("Audio request cancelled"));
        }
        Ok(())
    }

    fn finish(
        &mut self,
        operation: Operation<dyn FnMut(bool)>,
        success: Rc<Cell<bool>>,
    ) -> io::Result<()> {
        self.wait(operation)?;
        if !success.get() {
            return Err(io::Error::other("Could not change audio device"));
        }
        Ok(())
    }

    pub fn read(&mut self, source: bool) -> io::Result<Device> {
        let value = Rc::new(RefCell::new(None));
        let result = value.clone();
        if source {
            let operation = self.context.introspect().get_source_info_by_name(
                "@DEFAULT_SOURCE@",
                move |info| {
                    if let ListResult::Item(info) = info {
                        *result.borrow_mut() = Some(Device {
                            index: info.index,
                            channels: info.volume,
                            muted: info.mute,
                        });
                    }
                },
            );
            self.wait(operation)?;
        } else {
            let operation =
                self.context
                    .introspect()
                    .get_sink_info_by_name("@DEFAULT_SINK@", move |info| {
                        if let ListResult::Item(info) = info {
                            *result.borrow_mut() = Some(Device {
                                index: info.index,
                                channels: info.volume,
                                muted: info.mute,
                            });
                        }
                    });
            self.wait(operation)?;
        }
        let device = *value.borrow();
        device.ok_or_else(|| io::Error::other("Default audio device unavailable"))
    }

    pub fn set_sink_volume(&mut self, index: u32, channels: &ChannelVolumes) -> io::Result<()> {
        let success = Rc::new(Cell::new(false));
        let result = success.clone();
        let operation = self.context.introspect().set_sink_volume_by_index(
            index,
            channels,
            Some(Box::new(move |ok| result.set(ok))),
        );
        self.finish(operation, success)
    }

    pub fn set_mute(&mut self, source: bool, index: u32, mute: bool) -> io::Result<()> {
        let success = Rc::new(Cell::new(false));
        let result = success.clone();
        let callback: Option<Box<dyn FnMut(bool)>> = Some(Box::new(move |ok| result.set(ok)));
        let operation = if source {
            self.context
                .introspect()
                .set_source_mute_by_index(index, mute, callback)
        } else {
            self.context
                .introspect()
                .set_sink_mute_by_index(index, mute, callback)
        };
        self.finish(operation, success)
    }
}

impl Drop for Pulse {
    fn drop(&mut self) {
        if self.context.get_state().is_good() {
            self.context.disconnect();
        }
    }
}
