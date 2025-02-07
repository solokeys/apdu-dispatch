use apdu_dispatch::app::{App, CommandView, Result as AppResult};
use apdu_dispatch::dispatch;
use apdu_dispatch::{interchanges, response};
use hex_literal::hex;
use interchange::Channel;
use iso7816::Status;

#[macro_use]
extern crate serial_test;

#[macro_use]
extern crate delog;
generate_macros!();

#[derive(Debug)]
pub struct StdoutFlusher {}
impl delog::Flusher for StdoutFlusher {
    fn flush(&self, logs: &str) {
        print!("{logs}");
    }
}

delog!(Delogger, 25 * 1024, 25 * 1024, StdoutFlusher);

static STDOUT_FLUSHER: StdoutFlusher = StdoutFlusher {};

#[allow(dead_code)]
enum TestInstruction {
    Echo = 0x10,
    Add = 0x11,
    GetData = 0x12,
}

fn dump_hex(data: &[u8]) {
    for b in data {
        print!("{b:02X} ");
    }
    println!();
}

pub struct TestApp1 {}

impl iso7816::App for TestApp1 {
    fn aid(&self) -> iso7816::Aid {
        iso7816::Aid::new(&hex!("0A01000001"))
    }
}

// This app echos to Ins code 0x10
impl App<{ apdu_dispatch::response::SIZE }> for TestApp1 {
    fn select(
        &mut self,
        _interface: dispatch::Interface,
        _apdu: CommandView<'_>,
        _reply: &mut response::Data,
    ) -> AppResult {
        Ok(())
    }

    fn deselect(&mut self) {}

    fn call(
        &mut self,
        _: dispatch::Interface,
        apdu: CommandView<'_>,
        reply: &mut response::Data,
    ) -> AppResult {
        println!("TestApp1::call");
        match apdu.instruction().into() {
            0x10 => {
                // Just echo 5x 0's for the request apdu header
                reply.push(0).unwrap();
                reply.push(0).unwrap();
                reply.push(0).unwrap();
                reply.push(0).unwrap();
                reply.push(0).unwrap();
                reply.extend_from_slice(apdu.data()).unwrap();
                Ok(())
            }
            // For measuring the stack burden of dispatch
            0x15 => {
                let buf = heapless::Vec::new();
                let addr = (&buf as *const response::Data) as u32;
                reply.extend_from_slice(&addr.to_be_bytes()).unwrap();
                Ok(())
            }

            // Testing a response larger than the interchange's size
            0x21 => {
                reply
                    .extend_from_slice(&[10; interchanges::SIZE + 1])
                    .unwrap();
                Ok(())
            }
            0x22 => {
                reply
                    .extend_from_slice(&[10; interchanges::SIZE - 2])
                    .unwrap();
                Ok(())
            }
            0x23 => {
                reply
                    .extend_from_slice(&[10; interchanges::SIZE - 1])
                    .unwrap();
                Ok(())
            }
            0x24 => {
                reply.extend_from_slice(&[10; interchanges::SIZE]).unwrap();
                Ok(())
            }
            _ => Err(Status::InstructionNotSupportedOrInvalid),
        }
    }
}

pub struct TestApp2 {}

impl iso7816::App for TestApp2 {
    fn aid(&self) -> iso7816::Aid {
        iso7816::Aid::new(&hex!("0A01000002"))
    }
}

// This app echos to Ins code 0x20
impl App<{ apdu_dispatch::response::SIZE }> for TestApp2 {
    fn select(
        &mut self,
        _interface: dispatch::Interface,
        _apdu: CommandView<'_>,
        _reply: &mut response::Data,
    ) -> AppResult {
        Ok(())
    }

    fn deselect(&mut self) {}

    fn call(
        &mut self,
        _: dispatch::Interface,
        apdu: CommandView<'_>,
        reply: &mut response::Data,
    ) -> AppResult {
        println!("TestApp2::call");
        match apdu.instruction().into() {
            0x20 => {
                reply.push(0).unwrap();
                reply.push(0).unwrap();
                reply.push(0).unwrap();
                reply.push(0).unwrap();
                reply.push(0).unwrap();
                reply.extend_from_slice(apdu.data()).unwrap();
                Ok(())
            }
            0x30 => {
                // Return 2KB bytes of byte-truncated fibonacci
                reply.extend_from_slice(&[0, 1, 1]).unwrap();
                for i in 3..2048 {
                    let next = ((reply[i - 1] as u32 + reply[i - 2] as u32) & 0xff) as u8;
                    reply.push(next).unwrap();
                }
                Ok(())
            }
            _ => Err(Status::InstructionNotSupportedOrInvalid),
        }
    }
}

pub struct PanicApp {}

impl iso7816::App for PanicApp {
    fn aid(&self) -> iso7816::Aid {
        iso7816::Aid::new(&hex!("0A01000003"))
    }
}

// This app echos to Ins code 0x20
impl App<{ apdu_dispatch::response::SIZE }> for PanicApp {
    fn select(
        &mut self,
        _interface: dispatch::Interface,
        _apdu: CommandView<'_>,
        _reply: &mut response::Data,
    ) -> AppResult {
        panic!("Dont call the panic app");
    }

    fn deselect(&mut self) {
        panic!("Dont call the panic app");
    }

    fn call(
        &mut self,
        _: dispatch::Interface,
        _apdu: CommandView<'_>,
        _reply: &mut response::Data,
    ) -> AppResult {
        panic!("Dont call the panic app");
    }
}

fn run_apdus(apdu_response_pairs: &[&[u8]]) {
    assert!(!apdu_response_pairs.is_empty());
    assert!((apdu_response_pairs.len() & 1) == 0);

    Delogger::init_default(delog::LevelFilter::Info, &STDOUT_FLUSHER).ok();

    let contact = Channel::new();
    let (mut contact_requester, contact_responder) = contact
        .split()
        .expect("could not setup ccid ApduInterchange");

    let contactless = Channel::new();
    let (_contactless_requester, contactless_responder) = contactless
        .split()
        .expect("could not setup iso14443 ApduInterchange");

    let mut apdu_dispatch =
        apdu_dispatch::dispatch::ApduDispatch::new(contact_responder, contactless_responder);
    Delogger::flush();

    let mut app0 = PanicApp {};
    let mut app1 = TestApp1 {};
    let mut app2 = PanicApp {};
    let mut app3 = TestApp2 {};
    let mut app4 = PanicApp {};

    // for i in 0..apdu_response_pairs.len() {
    // print!("- ");
    // dump_hex(apdu_response_pairs[i]);
    // }
    for i in (0..apdu_response_pairs.len()).step_by(2) {
        let raw_req = apdu_response_pairs[i];
        let raw_expected_res = apdu_response_pairs[i + 1];

        // let command = Command::try_from(raw_req).unwrap();
        // let expected_response = Response::Data::from_slice(&raw_res);

        print!("<< ");
        dump_hex(raw_req);

        contact_requester
            .request(interchanges::Data::from_slice(raw_req).unwrap())
            .expect("could not deposit command");

        apdu_dispatch.poll(&mut [&mut app0, &mut app1, &mut app2, &mut app3, &mut app4]);
        Delogger::flush();

        let response = contact_requester.take_response().unwrap();

        print!(">> ");
        dump_hex(&response);

        if raw_expected_res != response.as_slice() {
            print!("expected: ");
            dump_hex(raw_expected_res);
            print!("got: ");
            dump_hex(&response);
            panic!("Expected responses do not match");
        }
    }
}

#[test]
#[serial]
fn malformed_apdus() {
    run_apdus(&[
        // Too short
        &hex!("00"),
        &hex!("6F00"),
        // Too short
        &hex!("0000"),
        &hex!("6F00"),
        // Too short
        &hex!("000000"),
        &hex!("6F00"),
        // Wrong length
        &hex!("0000000010010101"),
        &hex!("6F00"),
        // Extra data
        &hex!("000000000501010101010101010101010101"),
        &hex!("6F00"),
        // Invalid CLA
        &hex!("FF000000"),
        &hex!("6F00"),
        // Invalid extended length
        &hex!("00000000ff00050101010101"),
        &hex!("6F00"),
        // sanity check with Valid APDU with extended length
        &hex!("000000000000050101010101"),
        &hex!("6A82"),
    ])
}

#[test]
#[serial]
fn select_1() {
    run_apdus(&[
        // Select
        &hex!("00A40400 05 0A01000001"),
        // Ok
        &hex!("9000"),
    ])
}

#[test]
#[serial]
fn select_2() {
    run_apdus(&[
        // Select
        &hex!("00A40400 05 0A01000002"),
        // Ok
        &hex!("9000"),
    ])
}

#[test]
#[serial]
fn select_not_found() {
    run_apdus(&[
        // Select
        &hex!("00A40400 05 0A01000100"),
        // Not found
        &hex!("6A82"),
    ])
}

#[test]
#[serial]
fn select_not_found_still_selected() {
    run_apdus(&[
        // Select
        &hex!("00A40400 05 0A01000001"),
        // Ok
        &hex!("9000"),
        // Select
        &hex!("00A40400 05 0A01000100"),
        // Not found
        &hex!("6A82"),
        // Echo app is still selected
        &hex!("80100000 05 0102030405 00"),
        // Echo + Ok
        &hex!("0000000000 0102030405 9000"),
    ])
}

#[test]
#[serial]
fn select_bad_aid() {
    // Select with an incorrect AID shouldn't crash the application
    run_apdus(&[
        // Select 1
        &hex!("00A40400"),
        &hex!("6A80"),
    ])
}

#[test]
#[serial]
fn echo_1() {
    run_apdus(&[
        // Select
        &hex!("00A40400 05 0A01000001"),
        // Ok
        &hex!("9000"),
        // Echo
        &hex!("00100000 05 0102030405 00"),
        // Echo + Ok
        &hex!("0000000000 01020304059000"),
    ])
}

#[test]
#[serial]
fn echo_with_cla_bits_set() {
    run_apdus(&[
        // Select
        &hex!("00A40400 05 0A01000001"),
        // Ok
        &hex!("9000"),
        // Echo
        &hex!("80100000 05 0102030405 00"),
        // Echo + Ok
        &hex!("0000000000 0102030405 9000"),
    ])
}

#[test]
#[serial]
fn echo_wrong_instruction() {
    run_apdus(&[
        // Select
        &hex!("00A40400 05 0A01000001"),
        // Ok
        &hex!("9000"),
        // Echo
        &hex!("00200000 05 0102030405 00"),
        // Wrong Ins
        &hex!("6d00"),
    ])
}

#[test]
#[serial]
fn echo_2() {
    run_apdus(&[
        // Select
        &hex!("00A40400 05 0A01000002"),
        // Ok
        &hex!("9000"),
        // Echo
        &hex!("00200000 05 0102030405 00"),
        // Echo + Ok
        &hex!("0000000000 0102030405 9000"),
    ])
}

#[test]
#[serial]
fn echo_wrong_instruction_2() {
    run_apdus(&[
        // Select
        &hex!("00A40400 05 0A01000002"),
        // Ok
        &hex!("9000"),
        // Echo
        &hex!("00100000 05 0102030405 00"),
        // Wrong Ins
        &hex!("6d00"),
    ])
}

#[test]
#[serial]
fn unsolicited_instruction() {
    run_apdus(&[
        // Echo
        &hex!("00100000 05 0102030405"),
        // Not found
        &hex!("6a82"),
    ])
}

#[test]
#[serial]
fn deselect() {
    run_apdus(&[
        // Select 1
        &hex!("00A40400 05 0A01000001"),
        &hex!("9000"),
        // Echo 1
        &hex!("00100000 05 0102030405 00"),
        &hex!("0000000000 0102030405 9000"),
        // Select 2
        &hex!("00A40400 05 0A01000002"),
        &hex!("9000"),
        // Echo 1
        &hex!("00100000 05 0102030405 00"),
        &hex!("6d00"),
    ])
}

#[test]
#[serial]
fn extended_length_echo() {
    run_apdus(
        &[
            // Select 1
            &hex!("00A40400 05 0A01000001"),
            &hex!("9000"),

            // To be echo'd
            &hex!("00100000 000123
                /*        1                    8                       16                      24                      32 */
                /* 1  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 2  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 3  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 4  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 5  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 6  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 7  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 8  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 9  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 10 */  01 01 01
                0000
            "),
            // echo  Success
            &hex!("0000000000
                /*        1                    8                       16                      24                      32 */
                /* 1  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 2  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 3  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 4  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 5  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 6  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 7  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 8  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 9  */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                /* 10 */  01 01 01
                9000
            ")
        ]
    )
}

#[test]
#[serial]
fn chained_apdu_1() {
    run_apdus(
        &[
            // Select 1
            &hex!("00A40400 05 0A01000001"),
            &hex!("9000"),

            // Set chaining bit
            &hex!("10200000FF
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
            "),
            &hex!("9000"),

            // Set chaining bit
            &hex!("10200000FF
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
            "),
            &hex!("9000"),

            // Send last command
            &hex!("0010000020
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                00
            "),
            // Expect 0xff + 0xff + 0x20 + 5 == 547 bytes back
            // Echo chunk + remaining
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  00 00 00 00 00 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                6100
            "),

            // Get Response
            &hex!("00C00000 00"),
            // Echo chunk + remaining
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                6123
            "),

            // Get Response
            &hex!("00C00000 00"),
            // Echo chunk + success
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01
                9000
            "),

            // Get Response
            &hex!("00C00000 00"),
            // Error
            &hex!("6F00"),
        ]
    )
}

#[test]
#[serial]
fn chained_response() {
    run_apdus(
        &[
            // Select 1
            &hex!("00A40400 05 0A01000001"),
            &hex!("9000"),

            // Set chaining bit
            &hex!("10200000FF
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
            "),
            &hex!("9000"),

            // Set chaining bit
            &hex!("10200000FF
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
            "),
            &hex!("9000"),

            // Send last command
            &hex!("0010000040
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                23 // Ask for less than 256
            "),
            // Echo chunk + remaining
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  00 00 00 00 00 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01
                6100
            "),

            // Get Response
            &hex!("00C00000 00"),
            // Echo chunk + remaining
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                6100
            "),

            // Get Response
            &hex!("00C00000 00"),
            // Echo chunk + success
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                6120
            "),

            // Get Response
            &hex!("00C00000 00"),
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                9000
            "),

            // Get Response
            &hex!("00C00000 00"),
            // Error
            &hex!("6F00"),
        ]
    )
}

#[test]
#[serial]
fn multiple_chained_apdu_1() {
    run_apdus(
        &[
            // Select 1
            &hex!("00A40400 05 0A01000001"),
            &hex!("9000"),

            // Set chaining bit
            &hex!(" 10200000ff
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
            "),
            &hex!("9000"),


            // Send last command
            &hex!(" 0010000020
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                00
            "),
            // Expect 0xff + 0xff + 0x20 + 5 == 292 bytes back
            // Data + remaining bytes
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  00 00 00 00 00 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                6124
            "),

            // Get Response
            &hex!("00C00000 00"),
            // Echo chunk + success
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01
                9000
            "),

            // Check short commands still work
            // Echo 1
            &hex!("00100000 05 0102030405 00"),
            &hex!("0000000000 01020304059000"),

            // Echo 2
            &hex!("00200000 05 0102030405 00"),
            &hex!("6d00"),

            // Check chaining command still works
            // Set chaining bit
            &hex!("10200000FF
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
            "),
            &hex!("9000"),


            // Send last command
            &hex!("0010000020
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                00
            "),
            // Expect 0xff + 0xff + 0x20 + 5 == 292 bytes back
            // Data + remaining bytes
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  00 00 00 00 00 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
                6124
            "),

            // Get Response
            &hex!("00C00000 00"),
            // Echo chunk + success
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01
                9000
            "),
        ]
    )
}

#[test]
#[serial]
fn test_chained_fibonacci_response() {
    let mut expected = response::Data::new();
    expected.extend_from_slice(&[0, 1, 1]).unwrap();
    for i in 3..2048 {
        let next = ((expected[i - 1] as u32 + expected[i - 2] as u32) & 0xff) as u8;
        expected.push(next).unwrap();
    }
    // expected_reply.extend_from_slice(&[0x90, 0x00]).unwrap();
    fn apdu_res_chunk(data: &response::Data, start: &mut usize, size: usize) -> response::Data {
        let mut chunk = response::Data::new();
        let end = *start + size;
        chunk.extend_from_slice(&data[*start..end]).unwrap();
        if data[*start..].len() > 256 {
            chunk.push(0x61).unwrap();

            if data[end..].len() > 255 {
                chunk.push(0).unwrap();
            } else {
                chunk.push(data[end..].len() as u8).unwrap();
            }
        } else {
            chunk.push(0x90).unwrap();
            chunk.push(0x00).unwrap();
        }
        *start += size;
        chunk
    }

    let mut start = 0;
    let mut start2 = 0;

    run_apdus(
        &[
            // Select 2
            &hex!("00A40400 05 0A01000002"),
            &hex!("9000"),

            // Set chaining bit, command to get long fibonacci back
            &hex!("10300000FF
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
            "),
            &hex!("9000"),

            // Send last command
            &hex!("0030000020
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                00
            "),
            apdu_res_chunk(&expected, &mut start, 256).as_slice(),

            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start, 256).as_slice(),

            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start, 256).as_slice(),

            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start, 256).as_slice(),
            &hex!("00C00000 00"),

            apdu_res_chunk(&expected, &mut start, 256).as_slice(),
            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start, 256).as_slice(),
            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start, 256).as_slice(),
            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start, 256).as_slice(),

            // chaining bit, command to get long fibonacci back
            &hex!("10300000 05 0102030405"),
            &hex!("9000 "),

            &hex!("00300000 05 0102030405 00"),
            apdu_res_chunk(&expected, &mut start2, 256).as_slice(),

            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start2, 256).as_slice(),

            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start2, 256).as_slice(),

            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start2, 256).as_slice(),

            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start2, 256).as_slice(),

            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start2, 256).as_slice(),

            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start2, 256).as_slice(),

            &hex!("00C00000 00"),
            apdu_res_chunk(&expected, &mut start2, 256).as_slice(),

        ]
    )
}

#[test]
#[serial]
fn multiple_chained_apdu_interruption() {
    run_apdus(
        &[
            // Select 1
            &hex!("00A40400050A01000001"),
            &hex!("9000"),

            // Set chaining bit
            &hex!("10200000FF
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
            "),
            &hex!("9000"),


            // Send last command
            &hex!("0010000020
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                00
            "),
            // Expect 0xff + 0xff + 0x20 + 5 == 292 bytes back
            // Data + remaining bytes
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  00 00 00 00 00 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                6124
            "),
            // Just ignore those 36 bytes and do something different
            // Echo 1
            &hex!("00100000 05 0102030405 00"),
            &hex!("0000000000 0102030405 9000"),

            // GetResponse no longer works
            &hex!("00C00000"),
            &hex!("6F00 "),

            // Check that new chaining transaction works
            &hex!("10200000FF
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
            "),
            &hex!("9000"),


            // Send last command
            &hex!("0010000020
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                00
            "),
            // Expect 0xff + 0xff + 0x20 + 5 == 292 bytes back
            // Data + remaining bytes
            &hex!("
                /*       1                    8                       16                      24                      32 */
                /* 1 */  00 00 00 00 00 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                6124
            "),

        ]
    )
}

#[test]
#[serial]
fn chaining_with_unknown_class_range() {
    run_apdus(
        &[
            // Select 1
            &hex!("00A40400 05 0A01000001"),
            &hex!("9000"),

            // Set chaining bit + upper range bit
            &hex!("90200000FF
                /*       1                    8                       16                      24                      32 */
                /* 1 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 2 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 3 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 4 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 5 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 6 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 7 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 
                /* 8 */  01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01
            "),
            &hex!("9000"),
        ]
    )
}

#[test]
#[serial]
fn send_select_preceded_with_zero_chained_data() {
    // Sending a select after chaining 0 bytes should result in successful select operation
    run_apdus(&[
        // Chaining zero data
        &hex!("9060000000"),
        &hex!("9000"),
        // Select 1
        &hex!("00A40400 05 0A01000001"),
        &hex!("9000"),
    ])
}

#[test]
#[serial]
fn response_larger_than_interchange() {
    // Sending a select after chaining 0 bytes should result in successful select operation
    let mut response1 = vec![0x0A; interchanges::SIZE - 2];
    response1.extend_from_slice(&hex!("6103"));
    let mut response2 = vec![0x0A; interchanges::SIZE - 2];
    response2.extend_from_slice(&hex!("9000"));
    let mut response3 = vec![0x0A; interchanges::SIZE - 2];
    response3.extend_from_slice(&hex!("6101"));
    let mut response4 = vec![0x0A; interchanges::SIZE - 2];
    response4.extend_from_slice(&hex!("6102"));
    run_apdus(&[
        // Select 1
        &hex!("00A40400 05 0A01000001"),
        &hex!("9000"),
        &hex!("00210000  00ffff"),
        &response1,
        // Get Response
        &hex!("00C00000 00"),
        &hex!("0A0A0A 9000"),
        &hex!("00220000  00ffff"),
        &response2,
        &hex!("00230000  00ffff"),
        &response3,
        // Get Response
        &hex!("00C00000 00"),
        &hex!("0A 9000"),
        &hex!("00240000  00ffff"),
        &response4,
        // Get Response
        &hex!("00C00000 00"),
        &hex!("0A0A 9000"),
    ])
}

#[test]
#[serial]
fn check_stack_burden() {
    let contact = Channel::new();
    let (mut contact_requester, contact_responder) = contact
        .split()
        .expect("could not setup ccid ApduInterchange");

    let contactless = Channel::new();
    let (_contactless_requester, contactless_responder) = contactless
        .split()
        .expect("could not setup iso14443 ApduInterchange");

    let mut apdu_dispatch =
        apdu_dispatch::dispatch::ApduDispatch::new(contact_responder, contactless_responder);

    let mut app1 = TestApp1 {};

    contact_requester
        .request(interchanges::Data::from_slice(&hex!("00A40400050A01000001")).unwrap())
        .expect("could not deposit command");

    apdu_dispatch.poll(&mut [&mut app1]);

    let response = contact_requester.take_response().unwrap();

    print!(">> ");
    dump_hex(&response);

    contact_requester
        .request(interchanges::Data::from_slice(&hex!("0015000000")).unwrap())
        .expect("could not deposit command");

    apdu_dispatch.poll(&mut [&mut app1]);

    let response = contact_requester.take_response().unwrap();

    print!(">> ");
    dump_hex(&response);

    let payload: [u8; 4] = [response[0], response[1], response[2], response[3]];
    let min_stack = u32::from_be_bytes(payload);
    let max_stack = (&response as *const interchanges::Data) as u32;

    // Last checked:
    // Burden: 43744 bytes with Large apdu for Command & Response, not returning Data.
    println!("Burden: {} bytes", max_stack - min_stack);

    // Uncomment to see stack burden printed out
    // assert!(false);
}
