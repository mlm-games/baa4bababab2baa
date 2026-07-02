use std::thread;
use std::time::Duration;

use mediacodec::{BufferFlag, CodecOutputBuffer, MediaCodec};
use tokio::sync::oneshot;

use crate::error::Error;

pub(super) enum Cmd<T> {
    Item(T),
    Flush(oneshot::Sender<Result<(), Error>>),
    Close,
}

pub(super) fn send_eos(codec: &mut MediaCodec) -> Result<(), Error> {
    for _ in 0..5000 {
        if let Ok(buf) = codec.dequeue_input(0) {
            let mut buf: mediacodec::CodecInputBuffer = buf;
            buf.set_flags(BufferFlag::EndOfStream as u32);
            buf.set_write_size(0);
            buf.set_time(0);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(1));
    }
    Err(Error::Platform("send_eos timed out".into()))
}

pub(super) fn drain_until_eos<F>(codec: &mut MediaCodec, mut on_output: F) -> Result<(), Error>
where
    F: FnMut(CodecOutputBuffer) -> Result<(), Error>,
{
    let mut consecutive_empty = 0u32;
    for _ in 0..5000 {
        match codec.dequeue_output(1000) {
            Ok(out) => {
                consecutive_empty = 0;
                let out: CodecOutputBuffer = out;
                if BufferFlag::EndOfStream.is_contained_in(out.info().flags as i32) {
                    return Ok(());
                }
                on_output(out)?;
            }
            Err(_) => {
                consecutive_empty += 1;
                if consecutive_empty > 50 {
                    return Err(Error::Platform("drain_until_eos timed out".into()));
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
    Err(Error::Platform("drain_until_eos timed out".into()))
}
