//! This "APDU dispatch" consumes APDUs from either a contactless or contact interface, or both.
//! Each APDU will be sent to an "App".  The dispatch will manage selecting and deselecting apps,
//! and will gauruntee only one app will be selected at a time.  Only the selected app will
//! receive APDU's.  Apps are selected based on their AID.
//!
//! Additionally, the APDU dispatch could repeatedly call "poll" on the selected App.  If this was in place, the App
//! could choose to reply at time of APDU, or can defer and reply later (during one of the poll calls).
//!
//! Apps need to implement the App trait to be managed.
//!
use core::mem;

use crate::response::SIZE as ResponseSize;
use crate::App;
use crate::{
    interchanges::{self, Responder},
    response, Command,
};

use iso7816::{
    command::{CommandView, FromSliceError},
    Aid, Instruction, Result, Status,
};

/// Maximum length of a data field of a response that can fit in an interchange message after
/// concatenation of SW1SW2
const MAX_INTERCHANGE_DATA: usize = if interchanges::SIZE < ResponseSize {
    interchanges::SIZE
} else {
    ResponseSize
} - 2;

pub use iso7816::Interface;

pub enum RequestType {
    Select(Aid, Interface),
    /// Get Response including the Le field of the command
    GetResponse,
    NewCommand(Interface),
    /// Incorrect command, which means an error should be returned
    BadCommand(Status),
    None,
}

#[derive(PartialEq)]
enum RawApduBuffer {
    None,
    Request(Command),
    Response(response::Data),
}

struct ApduBuffer {
    pub raw: RawApduBuffer,
}

impl ApduBuffer {
    fn request(&mut self, command: CommandView<'_>) {
        match &mut self.raw {
            RawApduBuffer::Request(buffered) => {
                buffered.extend_from_command_view(command).ok();
            }
            _ => {
                if self.raw != RawApduBuffer::None {
                    info!("Was buffering the last response, but aborting that now for this new request.");
                }
                let mut new_cmd = iso7816::Command::try_from(&[0, 0, 0, 0]).unwrap();
                new_cmd.extend_from_command_view(command).ok();
                self.raw = RawApduBuffer::Request(new_cmd);
            }
        }
    }

    fn response(&mut self, response: &response::Data) {
        self.raw = RawApduBuffer::Response(response.clone());
    }
}

pub struct ApduDispatch<'pipe> {
    // or currently_selected_aid, or...
    current_aid: Option<Aid>,
    contact: Responder<'pipe>,
    contactless: Responder<'pipe>,
    interface: Option<Interface>,

    buffer: ApduBuffer,
    response_len_expected: usize,
    was_request_chained: bool,
}

impl<'pipe> ApduDispatch<'pipe> {
    fn apdu_type(apdu: CommandView<'_>, interface: Interface) -> RequestType {
        info!("instruction: {:?} {}", apdu.instruction(), apdu.p1);
        if apdu.instruction() == Instruction::Select && (apdu.p1 & 0x04) != 0 {
            Aid::try_new(apdu.data()).map_or_else(
                |_err| {
                    warn!("Failed to parse AID: {:?}", _err);
                    RequestType::BadCommand(Status::IncorrectDataParameter)
                },
                |aid| RequestType::Select(aid, interface),
            )
        } else if apdu.instruction() == Instruction::GetResponse {
            RequestType::GetResponse
        } else {
            RequestType::NewCommand(interface)
        }
    }

    pub fn new(contact: Responder<'pipe>, contactless: Responder<'pipe>) -> Self {
        ApduDispatch {
            current_aid: None,
            contact,
            contactless,
            interface: None,
            was_request_chained: false,
            response_len_expected: 0,
            buffer: ApduBuffer {
                raw: RawApduBuffer::None,
            },
        }
    }

    // It would be nice to store `current_app` instead of constantly looking up by AID,
    // but that won't work due to ownership rules
    fn find_app<'a, 'b>(
        aid: Option<&Aid>,
        apps: &'a mut [&'b mut dyn App<ResponseSize>],
    ) -> Option<&'a mut &'b mut dyn App<ResponseSize>> {
        // match aid {
        //     Some(aid) => apps.iter_mut().find(|app| aid.starts_with(app.rid())),
        //     None => None,
        // }
        aid.and_then(move |aid| {
            debug!("matching {:?}", aid);
            apps.iter_mut().find(|app| {
                // aid.starts_with(app.aid().truncated())
                debug!("...against {:?}", app.aid());
                app.aid().matches(aid)
            })
        })
    }

    fn busy(&self) -> bool {
        // the correctness of this relies on the properties of interchange - requester can only
        // send request in the idle state.
        use interchange::State::*;
        let contact_busy = !matches!(self.contact.state(), Idle | Requested);
        let contactless_busy = !matches!(self.contactless.state(), Idle | Requested);
        contactless_busy || contact_busy
    }

    #[inline(never)]
    fn buffer_chained_apdu_if_needed(
        &mut self,
        command: CommandView<'_>,
        interface: Interface,
    ) -> RequestType {
        // iso 7816-4 5.1.1
        // check Apdu level chaining and buffer if necessary.
        if !command.class().chain().not_the_last() {
            let is_chaining = matches!(self.buffer.raw, RawApduBuffer::Request(_));

            if is_chaining {
                self.buffer.request(command);

                // Response now needs to be chained.
                self.was_request_chained = true;
                info!("combined chained commands.");

                RequestType::NewCommand(interface)
            } else {
                if self.buffer.raw == RawApduBuffer::None {
                    self.was_request_chained = false;
                }
                let apdu_type = Self::apdu_type(command, interface);
                match apdu_type {
                    // Keep buffer the same in case of GetResponse
                    RequestType::GetResponse => (),
                    // Overwrite for everything else.
                    _ => self.buffer.request(command),
                }
                apdu_type
            }
        } else {
            match interface {
                // acknowledge
                Interface::Contact => {
                    self.contact
                        .respond(Status::Success.into())
                        .expect("Could not respond");
                }
                Interface::Contactless => {
                    self.contactless
                        .respond(Status::Success.into())
                        .expect("Could not respond");
                }
            }

            if !command.data().is_empty() {
                info!("chaining {} bytes", command.data().len());
                self.buffer.request(command);
            }

            // Nothing for the application to consume yet.
            RequestType::None
        }
    }

    fn parse_apdu<const S: usize>(message: &interchanges::Data) -> Result<iso7816::Command<S>> {
        debug!(">> {}", hex_str!(message.as_slice(), sep:""));
        match iso7816::Command::try_from(message) {
            Ok(command) => Ok(command),
            Err(_error) => {
                info!("apdu bad");
                match _error {
                    FromSliceError::TooShort => {
                        info!("TooShort");
                    }
                    FromSliceError::TooLong => {
                        info!("TooLong");
                    }
                    FromSliceError::InvalidClass => {
                        info!("InvalidClass");
                    }
                    FromSliceError::InvalidFirstBodyByteForExtended => {
                        info!("InvalidFirstBodyByteForExtended");
                    }
                    FromSliceError::InvalidSliceLength => {
                        info!("InvalidSliceLength");
                    }
                }
                Err(Status::UnspecifiedCheckingError)
            }
        }
    }

    #[inline(never)]
    fn check_for_request(&mut self) -> RequestType {
        if !self.busy() {
            // Check to see if we have gotten a message, giving priority to contactless.
            let (message, interface) = if let Some(message) = self.contactless.take_request() {
                (message, Interface::Contactless)
            } else if let Some(message) = self.contact.take_request() {
                (message, Interface::Contact)
            } else {
                return RequestType::None;
            };

            let apdu;

            if let Some(i) = self.interface {
                if i != interface {
                    apdu = Err(Status::UnspecifiedNonpersistentExecutionError)
                } else {
                    apdu = Self::parse_apdu::<{ interchanges::SIZE }>(&message);
                }
            } else {
                self.interface = Some(interface);
                apdu = Self::parse_apdu::<{ interchanges::SIZE }>(&message);
            }

            // Parse the message as an APDU.
            match apdu {
                Ok(command) => {
                    self.response_len_expected = command.expected();
                    // The Apdu may be standalone or part of a chain.
                    self.buffer_chained_apdu_if_needed(command.as_view(), interface)
                }
                Err(response) => {
                    // If not a valid APDU, return error and don't pass to app.
                    info!("Invalid apdu");
                    match interface {
                        Interface::Contactless => self
                            .contactless
                            .respond(response.into())
                            .expect("cant respond"),
                        Interface::Contact => {
                            self.contact.respond(response.into()).expect("cant respond")
                        }
                    }
                    RequestType::None
                }
            }
        } else {
            RequestType::None
        }
    }

    #[inline(never)]
    fn reply_error(&mut self, status: Status) {
        self.respond(status.into());
        self.buffer.raw = RawApduBuffer::None;
    }

    #[inline(never)]
    fn handle_reply(&mut self) {
        // Consider if we need to reply via chaining method.
        // If the reader is using chaining, we will simply
        // reply 61XX, and put the response in a buffer.
        // It is up to the reader to then send GetResponse
        // requests, to which we will return up to `Le` bytes at a time.
        let (new_state, response) = match &mut self.buffer.raw {
            RawApduBuffer::Request(_) | RawApduBuffer::None => {
                info!("Unexpected GetResponse request.");
                (RawApduBuffer::None, Status::UnspecifiedCheckingError.into())
            }
            RawApduBuffer::Response(res) => {
                let max_response_len = self.response_len_expected.min(MAX_INTERCHANGE_DATA);
                if self.was_request_chained || res.len() > max_response_len {
                    // Do not send more than the expected bytes
                    let boundary = max_response_len.min(res.len());

                    let to_send = &res[..boundary];
                    let remaining = &res[boundary..];
                    let mut message = interchanges::Data::from_slice(to_send).unwrap();
                    let return_code = if remaining.len() > 255 {
                        // XX = 00 indicates more than 255 bytes of data
                        0x6100u16
                    } else if !remaining.is_empty() {
                        0x6100 + (remaining.len() as u16)
                    } else {
                        // Last chunk has success code
                        0x9000
                    };
                    message
                        .extend_from_slice(&return_code.to_be_bytes())
                        .expect("Failed add to status bytes");
                    if return_code == 0x9000 {
                        (RawApduBuffer::None, message)
                    } else {
                        info!("Still {} bytes in response buffer", remaining.len());
                        (
                            RawApduBuffer::Response(response::Data::from_slice(remaining).unwrap()),
                            message,
                        )
                    }
                } else {
                    // Add success code
                    res.extend_from_slice(&[0x90, 00])
                        .expect("Failed to add the status bytes");
                    (
                        RawApduBuffer::None,
                        interchanges::Data::from_slice(res.as_slice()).unwrap(),
                    )
                }
            }
        };
        self.buffer.raw = new_state;
        self.respond(response);
    }

    #[inline(never)]
    fn handle_app_response(&mut self, response: &Result<()>, data: &response::Data) {
        // put message into the response buffer
        match response {
            Ok(()) => {
                info!("buffered the response of {} bytes.", data.len());
                self.buffer.response(data);
                self.handle_reply();
            }
            Err(status) => {
                // Just reply the error immediately.
                info!("buffered app error");
                self.reply_error(*status);
            }
        }
    }

    #[inline(never)]
    fn handle_app_select(
        &mut self,
        apps: &mut [&mut dyn App<ResponseSize>],
        aid: Aid,
        interface: Interface,
    ) {
        // three cases:
        // - currently selected app has different AID -> deselect it, to give it
        //   the chance to clear sensitive state
        // - currently selected app has given AID (typical behaviour will be NOP,
        //   but pass along anyway) -> do not deselect it first
        // - no currently selected app
        //
        // For PIV, "SELECT" is NOP if it was already selected, but this is
        // not necessarily the case for other apps

        // if there is a selected app with a different AID, deselect it

        // select specified app in any case
        if let Some(app) = Self::find_app(Some(&aid), apps) {
            info!("Selected app");
            let mut response = response::Data::new();
            let result = match &self.buffer.raw {
                RawApduBuffer::Request(apdu) => {
                    app.select(interface, apdu.as_view(), &mut response)
                }
                _ => panic!("Unexpected buffer state."),
            };

            let old_aid = mem::replace(&mut self.current_aid, Some(aid));
            if let Some(old_aid) = old_aid {
                if old_aid != aid {
                    let app = Self::find_app(self.current_aid.as_ref(), apps).unwrap();
                    // for now all apps will be happy with this.
                    app.deselect();
                }
            }

            self.handle_app_response(&result, &response);
        } else {
            info!("could not find app by aid: {}", hex_str!(&aid.as_bytes()));
            self.reply_error(Status::NotFound);
        };
    }

    #[inline(never)]
    fn handle_app_command(
        &mut self,
        apps: &mut [&mut dyn App<ResponseSize>],
        interface: Interface,
    ) {
        // if there is a selected app, send it the command
        let mut response = response::Data::new();
        if let Some(app) = Self::find_app(self.current_aid.as_ref(), apps) {
            let result = match &self.buffer.raw {
                RawApduBuffer::Request(apdu) => app.call(interface, apdu.as_view(), &mut response),
                _ => panic!("Unexpected buffer state."),
            };
            self.handle_app_response(&result, &response);
        } else {
            // TODO: correct error?
            self.reply_error(Status::NotFound);
        };
    }

    pub fn poll(&mut self, apps: &mut [&mut dyn App<ResponseSize>]) -> Option<Interface> {
        // Only take on one transaction at a time.
        let request_type = self.check_for_request();

        // if there is a new request:
        // - if it's a select, handle appropriately
        // - else pass it on to currently selected app
        // if there is no new request, poll currently selected app
        match request_type {
            // SELECT case
            RequestType::Select(aid, interface) => {
                info!("Select");
                self.handle_app_select(apps, aid, interface);
            }

            RequestType::GetResponse => {
                info!("GetResponse");
                self.handle_reply();
            }

            // command that is not a special command -- goes to app.
            RequestType::NewCommand(interface) => {
                info!("Command");
                self.handle_app_command(apps, interface);
            }
            RequestType::BadCommand(status) => {
                info!("Bad command");
                self.reply_error(status);
            }
            RequestType::None => {}
        }

        // slight priority to contactless.
        if self.contactless.state() == interchange::State::Responded {
            Some(Interface::Contactless)
        } else if self.contact.state() == interchange::State::Responded {
            Some(Interface::Contact)
        } else {
            None
        }
    }

    #[inline(never)]
    fn respond(&mut self, message: interchanges::Data) {
        debug!("<< {}", hex_str!(message.as_slice(), sep:""));
        match self.interface.unwrap() {
            Interface::Contactless => self.contactless.respond(message).expect("cant respond"),
            Interface::Contact => self.contact.respond(message).expect("cant respond"),
        }
    }
}
