use boxfnonce::SendBoxFnOnce;
use failure::Error;
use glib::{self, ObjectExt};
use gst::{self, ElementExt, BinExt, BinExtManual, GObjectExtManualGst, PadDirection, PadExt};
use gst_sdp;
use gst_webrtc::{self, WebRTCSDPType};
use media_stream::GStreamerMediaStream;
use servo_media_webrtc::*;
use servo_media_webrtc::WebRtcController as WebRtcThread;
use std::sync::{Arc, Mutex};

// TODO:
// - remove use of failure?
// - figure out purpose of glib loop

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum MediaType {
    Audio,
    Video,
}

#[derive(PartialEq, PartialOrd, Eq, Debug, Copy, Clone, Ord)]
#[allow(unused)]
enum AppState {
    Error = 1,
    ServerConnected,
    ServerRegistering = 2000,
    ServerRegisteringError,
    ServerRegistered,
    PeerConnecting = 3000,
    PeerConnectionError,
    PeerConnected,
    PeerCallNegotiating = 4000,
    PeerCallNegotiatingHaveLocal,
    PeerCallNegotiatingHaveRemote,
    PeerCallStarted,
    PeerCallError,
}

#[derive(Clone)]
pub struct GStreamerWebRtcController(Arc<Mutex<WebRtcControllerState>>);

macro_rules! assert_state {
    ($controller:ident, $state:ident, $condition:expr, $string:expr) => {
        {
            let $state = $controller.0.lock().unwrap().app_state;
            assert!($condition, $string);
        }
    }
}

impl WebRtcControllerBackend for GStreamerWebRtcController {
    fn add_ice_candidate(&self, candidate: IceCandidate) {
        let app_control = self.0.lock().unwrap();
        app_control
            .webrtc
            .as_ref()
            .unwrap()
            .emit("add-ice-candidate", &[&candidate.sdp_mline_index, &candidate.candidate])
            .unwrap();
    }

    fn set_remote_description(&self, desc: SessionDescription, cb: SendBoxFnOnce<'static, ()>) {
        assert_state!(self, state,
                      state == AppState::PeerCallNegotiating || state == AppState::PeerCallNegotiatingHaveLocal,
                      "Not ready to handle sdp");

        self.set_description(desc, false, cb);

        let mut app_control = self.0.lock().unwrap();
        if app_control.app_state == AppState::PeerCallNegotiating {
            app_control.app_state = AppState::PeerCallNegotiatingHaveRemote;
        } else {
            app_control.app_state = AppState::PeerCallStarted;
        }
    }

    fn set_local_description(&self, desc: SessionDescription, cb: SendBoxFnOnce<'static, ()>) {
        assert_state!(self, state,
                      state == AppState::PeerCallNegotiating || state == AppState::PeerCallNegotiatingHaveRemote,
                      "Not ready to handle sdp");

        self.set_description(desc, true, cb);

        let mut app_control = self.0.lock().unwrap();
        if app_control.app_state == AppState::PeerCallNegotiating {
            app_control.app_state = AppState::PeerCallNegotiatingHaveLocal;
        } else {
            app_control.app_state = AppState::PeerCallStarted;
        }
    }

    fn create_offer(&self, cb: SendBoxFnOnce<'static, (SessionDescription,)>) {

        let app_control_clone = self.clone();
        let this = self.0.lock().unwrap();
        let webrtc = this.webrtc.as_ref().unwrap();;
        let promise = gst::Promise::new_with_change_func(move |promise| {
            on_offer_or_answer_created(SdpType::Offer, app_control_clone, promise, cb);
        });

        webrtc.emit("create-offer", &[&None::<gst::Structure>, &promise]).unwrap();
    }

    fn create_answer(&self, cb: SendBoxFnOnce<'static, (SessionDescription,)>) {
        let app_control_clone = self.clone();
        let this = self.0.lock().unwrap();
        let webrtc = this.webrtc.as_ref().unwrap();;
        let promise = gst::Promise::new_with_change_func(move |promise| {
            on_offer_or_answer_created(SdpType::Answer, app_control_clone, promise, cb);
        });

        webrtc.emit("create-answer", &[&None::<gst::Structure>, &promise]).unwrap();
    } 

    fn add_stream(&self, stream: &MediaStream) {
        println!("adding a stream");
        let (pipeline, webrtc) = {
            let mut controller = self.0.lock().unwrap();
            (controller.pipeline.clone(), controller.webrtc.clone().unwrap())
        };
        let stream = stream.as_any().downcast_ref::<GStreamerMediaStream>().unwrap();
        stream.attach_to_pipeline(&pipeline, &webrtc);
        self.0.lock().unwrap().prepare_for_negotiation(self.clone());
    }

    fn configure(&self, stun_server: &str, policy: BundlePolicy) {
        let data = self.0.lock().unwrap();
        let webrtc = data.webrtc.as_ref().unwrap();
        webrtc.set_property_from_str("stun-server", stun_server);
        webrtc.set_property_from_str("bundle-policy", policy.as_str());
    }
}

impl GStreamerWebRtcController {
    fn process_new_stream(
        &self,
        values: &[glib::Value],
        pipe: &gst::Pipeline,
    ) -> Option<glib::Value> {
        let pad = values[1].get::<gst::Pad>().expect("not a pad??");
        if pad.get_direction() != PadDirection::Src {
            // Ignore outgoing pad notifications.
            return None;
        }
        on_incoming_stream(self, values, pipe)
    }

    fn set_description(&self, desc: SessionDescription, local: bool, cb: SendBoxFnOnce<'static, ()>) {
        let ty = match desc.type_ {
            SdpType::Answer => WebRTCSDPType::Answer,
            SdpType::Offer => WebRTCSDPType::Offer,
            SdpType::Pranswer => WebRTCSDPType::Pranswer,
            SdpType::Rollback => WebRTCSDPType::Rollback,
        };

        let kind = if local { "set-local-description" } else { "set-remote-description" };

        let app_control = self.0.lock().unwrap();
        let ret = gst_sdp::SDPMessage::parse_buffer(desc.sdp.as_bytes()).unwrap();
        let answer =
            gst_webrtc::WebRTCSessionDescription::new(ty, ret);
        let promise = gst::Promise::new_with_change_func(move |_promise| {
            cb.call()
        });
        app_control
            .webrtc
            .as_ref()
            .unwrap()
            .emit(kind, &[&answer, &promise])
            .unwrap();
    }

    //#[allow(unused)]
    fn send_bus_error(&self, body: &str) {
        eprintln!("Bus error: {}", body);
        /*let mbuilder =
            gst::Message::new_application(gst::Structure::new("error", &[("body", &body)]));
        let _ = self.0.lock().unwrap().bus.post(&mbuilder.build());*/
        //XXXjdm
    }

    #[allow(unused)]
    fn update_state(&self, state: AppState) {
        self.0.lock().unwrap().update_state(state);
    }

    #[allow(unused)]
    fn close_and_quit(&self, err: &Error) {
        println!("{}\nquitting", err);

        // Must not hold mutex while shutting down the pipeline
        // as something might call into here and take the mutex too
        let pipeline = {
            let app_control = self.0.lock().unwrap();
            app_control.signaller.close(err.to_string());
            app_control.pipeline.clone()
        };

        pipeline.set_state(gst::State::Null).into_result().unwrap();

        //main_loop.quit();
    }
}

struct WebRtcControllerState {
    webrtc: Option<gst::Element>,
    app_state: AppState,
    pipeline: gst::Pipeline,
    thread: WebRtcThread,
    signaller: Box<WebRtcSignaller>,
    ready_to_negotiate: bool,
    //send_msg_tx: mpsc::Sender<OwnedMessage>,
    //peer_id: String,
    _main_loop: glib::MainLoop,
    //bus: gst::Bus,
}

impl WebRtcControllerState {
    fn prepare_for_negotiation(&mut self, target: GStreamerWebRtcController) {
        if self.ready_to_negotiate {
            return;
        }
        self.ready_to_negotiate = true;
        let webrtc = self.webrtc.as_ref().unwrap();
        // If the pipeline starts playing and this signal is present before there are any
        // media streams, an invalid SDP offer will be created. Therefore, delay setting up
        // the signal and starting the pipeline until after the first stream has been added.
        webrtc.connect("on-negotiation-needed", false, move |_values| {
            println!("on-negotiation-needed");
            let mut control = target.0.lock().unwrap();
            control.app_state = AppState::PeerCallNegotiating;
            control.signaller.on_negotiation_needed(&control.thread);
            None
        }).unwrap();
        self.pipeline.set_state(gst::State::Playing).into_result().unwrap();
    }

    fn start_pipeline(&mut self, target: GStreamerWebRtcController) {
        let webrtc = gst::ElementFactory::make("webrtcbin", "sendrecv").unwrap();
        self.pipeline.add(&webrtc).unwrap();

        let app_control_clone = target.clone();
        webrtc.connect("on-ice-candidate", false, move |values| {
            println!("on-ice-candidate");
            send_ice_candidate_message(&app_control_clone, values);
            None
        }).unwrap();

        let pipe_clone = self.pipeline.clone();
        let app_control_clone = target.clone();
        webrtc.connect("pad-added", false, move |values| {
            println!("pad-added");
            app_control_clone.process_new_stream(
                values,
                &pipe_clone,
            )
        }).unwrap();

        self.webrtc = Some(webrtc);
    }

    fn update_state(&mut self, state: AppState) {
        self.app_state = state;
    }
}

pub fn construct(
    signaller: Box<WebRtcSignaller>,
    thread: WebRtcThread,
) -> GStreamerWebRtcController {
    let main_loop = glib::MainLoop::new(None, false);
    let pipeline = gst::Pipeline::new("main");

    let controller = WebRtcControllerState {
        webrtc: None,
        pipeline,
        signaller,
        thread,
        app_state: AppState::ServerConnected,
        ready_to_negotiate: false,
        _main_loop: main_loop,
    };
    let controller = GStreamerWebRtcController(Arc::new(Mutex::new(controller)));
    controller.0.lock().unwrap().start_pipeline(controller.clone());
    controller
    
}

fn on_offer_or_answer_created(
    ty: SdpType,
    app_control: GStreamerWebRtcController,
    promise: &gst::Promise,
    cb: SendBoxFnOnce<'static, (SessionDescription,)>,
) {
    debug_assert!(ty == SdpType::Offer || ty == SdpType::Answer);
    if ty == SdpType::Offer {
        assert_state!(app_control, state, state == AppState::PeerCallNegotiating,
                      "Not negotiating call when creating offer")
    } else {
        assert_state!(app_control, state, state == AppState::PeerCallNegotiatingHaveRemote,
                      "No offfer received when creating answer")
    }

    let reply = promise.get_reply().unwrap();

    let reply = reply
        .get_value(ty.as_str())
        .unwrap()
        .get::<gst_webrtc::WebRTCSessionDescription>()
        .expect("Invalid argument");

    let type_ = match reply.get_type() {
        WebRTCSDPType::Answer => SdpType::Answer,
        WebRTCSDPType::Offer => SdpType::Offer,
        WebRTCSDPType::Pranswer => SdpType::Pranswer,
        WebRTCSDPType::Rollback => SdpType::Rollback,
        _ => panic!("unknown sdp response")
    };

    let desc = SessionDescription {
        sdp: reply.get_sdp().as_text().unwrap(),
        type_,
    };
    cb.call(desc);
}

fn handle_media_stream(
    pad: &gst::Pad,
    pipe: &gst::Pipeline,
    media_type: MediaType,
) -> Result<(), Error> {
    println!("Trying to handle stream {:?}", media_type);

    let (q, conv, sink) = match media_type {
        MediaType::Audio => {
            let q = gst::ElementFactory::make("queue", None).unwrap();
            let conv = gst::ElementFactory::make("audioconvert", None).unwrap();
            let sink = gst::ElementFactory::make("autoaudiosink", None).unwrap();
            let resample = gst::ElementFactory::make("audioresample", None).unwrap();

            pipe.add_many(&[&q, &conv, &resample, &sink])?;
            gst::Element::link_many(&[&q, &conv, &resample, &sink])?;

            resample.sync_state_with_parent()?;

            (q, conv, sink)
        }
        MediaType::Video => {
            let q = gst::ElementFactory::make("queue", None).unwrap();
            let conv = gst::ElementFactory::make("videoconvert", None).unwrap();
            let sink = gst::ElementFactory::make("autovideosink", None).unwrap();

            pipe.add_many(&[&q, &conv, &sink])?;
            gst::Element::link_many(&[&q, &conv, &sink])?;

            (q, conv, sink)
        }
    };
    q.sync_state_with_parent()?;
    conv.sync_state_with_parent()?;
    sink.sync_state_with_parent()?;

    let qpad = q.get_static_pad("sink").unwrap();
    pad.link(&qpad).into_result()?;

    Ok(())
}

fn on_incoming_decodebin_stream(
    app_control: &GStreamerWebRtcController,
    values: &[glib::Value],
    pipe: &gst::Pipeline,
) -> Option<glib::Value> {
    let pad = values[1].get::<gst::Pad>().expect("Invalid argument");
    if !pad.has_current_caps() {
        println!("Pad {:?} has no caps, can't do anything, ignoring", pad);
        return None;
    }

    let caps = pad.get_current_caps().unwrap();
    let name = caps.get_structure(0).unwrap().get_name();

    let handled = if name.starts_with("video") {
        handle_media_stream(&pad, &pipe, MediaType::Video)
    } else if name.starts_with("audio") {
        handle_media_stream(&pad, &pipe, MediaType::Audio)
    } else {
        println!("Unknown pad {:?}, ignoring", pad);
        Ok(())
    };

    if let Err(err) = handled {
        app_control.send_bus_error(&format!("Error adding pad with caps {} {:?}", name, err));
    }

    None
}

fn on_incoming_stream(
    app_control: &GStreamerWebRtcController,
    values: &[glib::Value],
    pipe: &gst::Pipeline,
) -> Option<glib::Value> {
    let webrtc = values[0].get::<gst::Element>().expect("Invalid argument");

    let decodebin = gst::ElementFactory::make("decodebin", None).unwrap();
    let pipe_clone = pipe.clone();
    let app_control_clone = app_control.clone();
    decodebin
        .connect("pad-added", false, move |values| {
            println!("decodebin pad-added");
            on_incoming_decodebin_stream(&app_control_clone, values, &pipe_clone)
        })
        .unwrap();

    pipe.add(&decodebin).unwrap();

    decodebin.sync_state_with_parent().unwrap();
    webrtc.link(&decodebin).unwrap();

    None
}

fn send_ice_candidate_message(app_control: &GStreamerWebRtcController, values: &[glib::Value]) {
    assert_state!(app_control, state, state >= AppState::PeerCallNegotiating, "Can't send ICE, not in call");

    let _webrtc = values[0].get::<gst::Element>().expect("Invalid argument");
    let sdp_mline_index = values[1].get::<u32>().expect("Invalid argument");
    let candidate = values[2].get::<String>().expect("Invalid argument");

    let candidate = IceCandidate {
        sdp_mline_index,
        candidate,
    };
    let control = app_control.0.lock().unwrap();
    control.signaller.on_ice_candidate(&control.thread, candidate);
}
