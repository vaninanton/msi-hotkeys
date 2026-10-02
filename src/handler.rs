//! Taking the buttons from the firmware and acting on the events they raise.

use anyhow::Result;

use crate::ec::{map, Machine};
use crate::{actions, buttons, journal, paths, readings, win};

/// Takes the buttons from the firmware, then blocks, handling events until the
/// process ends.
pub fn listen() -> Result<()> {
    let machine = Machine::open()?;

    match machine.set_handover(true) {
        Ok(true) => println!("took the buttons ({} -> 1)", map::HANDOVER.class),
        Ok(false) => println!("the buttons were already handed over"),
        // Worth continuing: the handover may have happened earlier in this
        // boot, in which case the events arrive anyway.
        Err(e) => eprintln!("WARNING: could not take the buttons: {e:#}"),
    }

    if let Err(e) = actions::write_template() {
        eprintln!("could not create the actions file: {e:#}");
    }

    println!("actions: {}", paths::actions().display());
    println!("listening for MSI_Event; log: {}", paths::log().display());

    for indication in machine.events()? {
        match indication {
            Ok(event) => handle(&machine, event.code),
            Err(e) => eprintln!("subscription error: {e:#}"),
        }
    }

    Ok(())
}

/// Everything that happens on a button press.
///
/// Nothing is done about the fan: Cooler Boost is carried out by the controller
/// before the event even arrives. The volume keys are left alone too, because
/// Windows already handles them over HID and acting on them here would double
/// the keypress — which is why the kernel driver ignores them as well.
fn handle(machine: &Machine, code: u32) {
    let name = buttons::name(code);
    println!("0x{code:06X}  {name}");

    // One reading serves both log lines. An earlier version took eleven WMI
    // round trips per press to produce the same two.
    let ec = machine.read();
    let status = readings::summary(&ec, machine.power().as_ref());
    journal::append(&format!("0x{code:06X}  {name:16}  {status}"));
    journal::append(&format!("{:26}{}", "", readings::dump(&ec)));

    // The actions file is read per press rather than at startup, so editing it
    // takes effect without restarting the handler.
    match actions::load().get(&code) {
        Some(command) => match actions::run(command) {
            Ok(()) => journal::append(&format!("{:26}ran: {command}", "")),
            Err(e) => journal::append(&format!("{:26}FAILED to run {command}: {e:#}", "")),
        },
        // Nothing mapped yet, so a free button at least reports the machine.
        None if buttons::FREE.contains(&code) => win::notify("msi-hotkeys", &status),
        None => {}
    }
}

/// Hands the buttons back to the firmware. Called on the way out, from whichever
/// thread is leaving, so it opens a connection of its own.
pub fn release_buttons() {
    match Machine::open().and_then(|machine| machine.set_handover(false)) {
        Ok(true) => println!("handed the buttons back"),
        Ok(false) => println!("the firmware already had them"),
        Err(e) => eprintln!("could not hand the buttons back: {e:#}"),
    }
}
