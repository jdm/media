extern crate byte_slice_cast;
extern crate failure;
extern crate glib;
extern crate gstreamer as gst;
extern crate gstreamer_app as gst_app;
extern crate gstreamer_audio as gst_audio;
extern crate gstreamer_player as gst_player;
extern crate gstreamer_sdp as gst_sdp;
extern crate gstreamer_video as gst_video;
extern crate gstreamer_webrtc as gst_webrtc;
extern crate ipc_channel;
#[macro_use]
extern crate lazy_static;

extern crate servo_media_audio;
extern crate servo_media_player;
extern crate servo_media_webrtc;

use servo_media_audio::sink::AudioSinkError;
use servo_media_audio::AudioBackend;
use servo_media_player::PlayerBackend;
use servo_media_webrtc::{WebRtcBackend, WebRtcSignaller};

pub mod audio_decoder;
pub mod audio_sink;
pub mod player;
pub mod webrtc;

pub struct GStreamerBackend;

impl AudioBackend for GStreamerBackend {
    type Decoder = audio_decoder::GStreamerAudioDecoder;
    type Sink = audio_sink::GStreamerAudioSink;
    fn make_decoder() -> Self::Decoder {
        audio_decoder::GStreamerAudioDecoder::new()
    }
    fn make_sink() -> Result<Self::Sink, AudioSinkError> {
        audio_sink::GStreamerAudioSink::new()
    }
}

impl PlayerBackend for GStreamerBackend {
    type Player = player::GStreamerPlayer;
    fn make_player() -> Self::Player {
        player::GStreamerPlayer::new()
    }
}

impl WebRtcBackend for GStreamerBackend {
    type Controller = webrtc::GStreamerWebRtcController;
    fn start_webrtc_controller(signaller: Box<WebRtcSignaller>) -> Self::Controller {
        webrtc::start(signaller)
    }
}

impl GStreamerBackend {
    pub fn init() {
        gst::init().unwrap();
    }
}
