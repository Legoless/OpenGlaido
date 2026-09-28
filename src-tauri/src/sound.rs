use rodio::source::SineWave;
use rodio::{OutputStream, Sink, Source};
use std::thread;
use std::time::Duration;

pub fn play_start_tone() {
    thread::spawn(|| {
        if let Ok((_stream, stream_handle)) = OutputStream::try_default() {
            if let Ok(sink) = Sink::try_new(&stream_handle) {
                // Short subtle ascending chime (587Hz D5 -> 880Hz A5)
                let tone1 = SineWave::new(587.33)
                    .take_duration(Duration::from_millis(45))
                    .amplify(0.12);
                let tone2 = SineWave::new(880.0)
                    .take_duration(Duration::from_millis(60))
                    .amplify(0.12);

                sink.append(tone1);
                sink.append(tone2);
                sink.sleep_until_end();
            }
        }
    });
}

pub fn play_stop_tone() {
    thread::spawn(|| {
        if let Ok((_stream, stream_handle)) = OutputStream::try_default() {
            if let Ok(sink) = Sink::try_new(&stream_handle) {
                // Short subtle descending tone (880Hz A5 -> 587Hz D5)
                let tone1 = SineWave::new(880.0)
                    .take_duration(Duration::from_millis(45))
                    .amplify(0.10);
                let tone2 = SineWave::new(587.33)
                    .take_duration(Duration::from_millis(60))
                    .amplify(0.10);

                sink.append(tone1);
                sink.append(tone2);
                sink.sleep_until_end();
            }
        }
    });
}
