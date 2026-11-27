//! PCM sources the output engine pulls from.

use audio_cache::AudioCache;

/// Interleaved PCM the audio callback pulls from.
///
/// Implementations are called on the real-time audio thread: `read_f32` must not block for long
/// or allocate.
pub trait PcmSource: Send + Sync + 'static {
    /// Source sample rate in Hz.
    fn sample_rate(&self) -> u32;
    /// Number of interleaved channels.
    fn channels(&self) -> u16;
    /// Total length in frames.
    fn total_frames(&self) -> u64;
    /// Reads interleaved frames starting at `start_frame` into `out` as `f32` in `[-1, 1]`.
    ///
    /// `out.len()` is a multiple of [`PcmSource::channels`]. Returns the number of frames
    /// written; frames past the end (or not available) are left to the caller to zero.
    fn read_f32(&self, start_frame: u64, out: &mut [f32]) -> usize;
}

impl PcmSource for AudioCache<f32> {
    fn sample_rate(&self) -> u32 {
        self.config().sample_rate
    }

    fn channels(&self) -> u16 {
        self.config().channels
    }

    fn total_frames(&self) -> u64 {
        self.config().total_frames
    }

    fn read_f32(&self, start_frame: u64, out: &mut [f32]) -> usize {
        self.read_frames(start_frame, out).unwrap_or(0)
    }
}

/// Stack scratch size (samples) used to convert `i16` blocks without heap allocation.
const I16_SCRATCH: usize = 4096;

impl PcmSource for AudioCache<i16> {
    fn sample_rate(&self) -> u32 {
        self.config().sample_rate
    }

    fn channels(&self) -> u16 {
        self.config().channels
    }

    fn total_frames(&self) -> u64 {
        self.config().total_frames
    }

    fn read_f32(&self, start_frame: u64, out: &mut [f32]) -> usize {
        let ch = usize::from(self.config().channels.max(1));
        let mut scratch = [0i16; I16_SCRATCH];
        let step_frames = I16_SCRATCH / ch;
        let total = out.len() / ch;
        let mut done = 0usize;
        while done < total {
            let want = step_frames.min(total - done);
            let n = self
                .read_frames(start_frame + done as u64, &mut scratch[..want * ch])
                .unwrap_or(0);
            if n == 0 {
                break;
            }
            for (dst, &s) in out[done * ch..(done + n) * ch]
                .iter_mut()
                .zip(&scratch[..n * ch])
            {
                *dst = f32::from(s) * (1.0 / 32_768.0);
            }
            done += n;
            if n < want {
                break;
            }
        }
        done
    }
}
