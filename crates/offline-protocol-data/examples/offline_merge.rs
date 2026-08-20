//! What happens when two people edit the same document while apart.
//!
//! Run it with:
//!
//! ```text
//! cargo run --package offline-protocol-data --example offline_merge
//! ```
//!
//! Two replicas start from one shared state, lose contact, both keep editing,
//! and then exchange what each committed while away. The point of the example
//! is the last line: after the exchange both replicas hold the same state, and
//! neither of them ran a conflict resolver an application had to write.
//!
//! This is the engine seam rather than the application API. An application
//! calls `data_map_set` and friends on `OfflineProtocol` and never sees a
//! delta, because the protocol carries them; see
//! `cargo run --package offline-protocol --example replicated_notes` for that
//! side. What this example is for is the question the other one cannot answer:
//! which edit wins, and what an application should therefore put in one key.

use offline_protocol_data::{DataDoc, DataValue};

fn text(value: &str) -> DataValue {
    DataValue::Text {
        value: value.to_string(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // One shared starting point: Ada writes, and Bo receives that change.
    let mut ada = DataDoc::new();
    ada.map_set("meta", "title", text("Coast road"))?;
    ada.list_push("packing", text("water"))?;
    let shared = ada.commit()?.expect("a change to share");

    let mut bo = DataDoc::new();
    bo.import(&shared.bytes)?;

    println!("both start at: {}", bo.export_json()?);

    // The link goes away here. Neither side can see the other's edits, and
    // neither side blocks on that.
    ada.map_set("meta", "title", text("Coast road, Friday"))?;
    ada.list_push("packing", text("map"))?;
    ada.text_insert("notes", 0, "leave at six")?;
    ada.counter_increment("edits", 1.0)?;
    let from_ada = ada.commit()?.expect("Ada's offline work");

    bo.map_set("meta", "title", text("Coast road, Saturday"))?;
    bo.list_push("packing", text("torch"))?;
    bo.text_insert("notes", 0, "bring the tent, ")?;
    bo.counter_increment("edits", 1.0)?;
    let from_bo = bo.commit()?.expect("Bo's offline work");

    println!("apart, Ada has: {}", ada.export_json()?);
    println!("apart, Bo has:  {}", bo.export_json()?);

    // The link comes back. Each side imports what the other committed. Order
    // does not matter and neither does arriving twice: importing the same
    // bytes again is a no-op, which is why at-least-once delivery is enough.
    ada.import(&from_bo.bytes)?;
    bo.import(&from_ada.bytes)?;
    bo.import(&from_ada.bytes)?;

    let ada_state = ada.export_json()?;
    let bo_state = bo.export_json()?;

    println!("together, Ada: {ada_state}");
    println!("together, Bo:  {bo_state}");
    println!("identical:     {}", ada_state == bo_state);

    // Read that state against the four collection types:
    //
    // - `meta.title` is a map key two people wrote. One value wins, both
    //   replicas pick the same one, and neither ends up holding half of each.
    //   That is the reason to put independently editable fields in separate
    //   keys rather than in one JSON blob.
    // - `packing` kept every insertion. A list loses nothing to a concurrent
    //   push.
    // - `notes` merged at character level, so both sentences survive.
    // - `edits` is 2. Two offline increments of 1 add up rather than
    //   overwriting each other, which is what makes a counter a counter and
    //   not an integer in a map.
    println!("packing:       {} entries", ada.list_len("packing")?);
    println!("notes:         {:?}", ada.text_value("notes")?);
    println!("edits:         {}", ada.counter_value("edits")?);

    Ok(())
}
