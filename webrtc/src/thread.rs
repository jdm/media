use std::sync::mpsc::{channel, Sender};
use std::thread;

use boxfnonce::SendBoxFnOnce;

use crate::{BundlePolicy, IceCandidate, MediaStream, SessionDescription};
use crate::{WebRtcControllerBackend, WebRtcSignaller, WebRtcBackend};

#[derive(Clone)]
/// Entry point for all client webrtc interactions.
pub struct WebRtcController {
    sender: Sender<RtcThreadEvent>,
}

impl WebRtcController {
    pub fn new<T: WebRtcBackend>(signaller: Box<WebRtcSignaller>) -> Self {
        let (sender, receiver) = channel();

        let t = WebRtcController { sender };

        let controller = T::construct_webrtc_controller(signaller, t.clone());

        thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                handle_rtc_event(&controller, event)
            }
        });

        t
    }
    pub fn configure(&self, stun_server: String, policy: BundlePolicy) {
        let _ = self.sender.send(RtcThreadEvent::ConfigureStun(stun_server, policy));
    }
    pub fn set_remote_description(&self, desc: SessionDescription, cb: SendBoxFnOnce<'static, ()>) {
        let _ = self.sender.send(RtcThreadEvent::SetRemoteDescription(desc, cb));
    }
    pub fn set_local_description(&self, desc: SessionDescription, cb: SendBoxFnOnce<'static, ()>) {
        let _ = self.sender.send(RtcThreadEvent::SetLocalDescription(desc, cb));
    }
    pub fn add_ice_candidate(&self, candidate: IceCandidate) {
        let _ = self.sender.send(RtcThreadEvent::AddIceCandidate(candidate));
    }
    pub fn create_offer(&self, cb: SendBoxFnOnce<'static, (SessionDescription,)>) {
        let _ = self.sender.send(RtcThreadEvent::CreateOffer(cb));
    }
    pub fn create_answer(&self, cb: SendBoxFnOnce<'static, (SessionDescription,)>) {
        let _ = self.sender.send(RtcThreadEvent::CreateAnswer(cb));
    }
    pub fn add_stream(&self, stream: Box<MediaStream>) {
        let _ = self.sender.send(RtcThreadEvent::AddStream(stream));
    }
}

pub enum RtcThreadEvent {
    ConfigureStun(String, BundlePolicy),
    SetRemoteDescription(SessionDescription, SendBoxFnOnce<'static, ()>),
    SetLocalDescription(SessionDescription, SendBoxFnOnce<'static, ()>),
    AddIceCandidate(IceCandidate),
    CreateOffer(SendBoxFnOnce<'static, (SessionDescription,)>),
    CreateAnswer(SendBoxFnOnce<'static, (SessionDescription,)>),
    AddStream(Box<MediaStream>),
}

pub fn handle_rtc_event(controller: &WebRtcControllerBackend, event: RtcThreadEvent) {
    match event {
        RtcThreadEvent::ConfigureStun(server, policy) => controller.configure(&server, policy),
        RtcThreadEvent::SetRemoteDescription(desc, cb) => {
            controller.set_remote_description(desc, cb)
        }
        RtcThreadEvent::SetLocalDescription(desc, cb) => controller.set_local_description(desc, cb),
        RtcThreadEvent::AddIceCandidate(candidate) => controller.add_ice_candidate(candidate),
        RtcThreadEvent::CreateOffer(cb) => controller.create_offer(cb),
        RtcThreadEvent::CreateAnswer(cb) => controller.create_answer(cb),
        RtcThreadEvent::AddStream(media) => controller.add_stream(&*media),
    }
}
