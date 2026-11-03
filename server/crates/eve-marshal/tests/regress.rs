//! Q-FUZZ regressions for `eve-marshal`: one test per crash or unbounded cost
//! the fuzzer found, each holding the (minimised) input.

use std::time::{Duration, Instant};

use eve_marshal::tag::code as c;
use eve_marshal::value::{Budget, DECODE_STACK};
use eve_marshal::{Decoder, PyValue, Reader, WireError, take_stream};

fn on_big_stack<F: FnOnce() + Send + 'static>(f: F) {
    std::thread::Builder::new()
        .stack_size(2 * DECODE_STACK)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

fn height(v: &PyValue) -> usize {
    let mut best = 0;
    let mut stack = vec![(v, 1usize)];
    while let Some((v, d)) = stack.pop() {
        best = best.max(d);
        match v {
            PyValue::Tuple(xs) | PyValue::List(xs) => stack.extend(xs.iter().map(|x| (x, d + 1))),
            _ => {}
        }
    }
    best
}

/// A `0x1B` back-reference clones its target in place, and the clone's levels
/// were never counted against the depth limit. Stage 1 registers a list nested
/// `per` deep; stage 2 registers a list nested `per` deep around a reference to
/// stage 1; and so on. 1,826 bytes built a value 1,804 deep, and each further
/// stage added `per` more for about `per` bytes, so a few hundred KB built one
/// hundreds of thousands deep: the recursive clone, drop and encode of it is a
/// stack overflow, which aborts the process. The decoder now remembers each
/// shared slot's height and refuses a reference that would pass the client's
/// limit of 1000.
fn sharing_stream(stages: usize, per: usize) -> Vec<u8> {
    let mut body = vec![c::TUPLE, 0xFF];
    body.extend_from_slice(&(stages as i32).to_le_bytes());
    for st in 0..stages {
        body.extend_from_slice(&[c::LIST | 0x40, 1]);
        body.extend(std::iter::repeat_n(c::LIST1, per));
        if st == 0 {
            body.push(c::NONE);
        } else {
            body.extend_from_slice(&[c::SHARED_REF, st as u8]);
        }
    }
    let mut s = vec![0x7E];
    s.extend_from_slice(&(stages as i32).to_le_bytes());
    s.extend_from_slice(&body);
    for id in 1..=stages as u32 {
        s.extend_from_slice(&id.to_le_bytes());
    }
    s
}

fn load(bytes: &[u8], budget: Option<Budget>) -> Result<PyValue, WireError> {
    let st = take_stream(bytes)?;
    let mut d = Decoder::with_shared(st.shared_ids);
    if let Some(b) = budget {
        d = d.with_budget(b);
    }
    d.load(&mut Reader::new(st.body))
}

#[test]
fn a_reference_cannot_stack_a_shared_value_past_the_depth_limit() {
    on_big_stack(|| {
        // two stages of 900: 1,804 deep once cloned
        let bytes = sharing_stream(2, 900);
        assert!(
            bytes.len() < 2000,
            "the input is small: {} bytes",
            bytes.len()
        );
        for budget in [Some(Budget::INBOUND), None] {
            assert_eq!(
                load(&bytes, budget),
                Err(WireError::Malformed {
                    at: 1811,
                    why: "recursion limit"
                })
            );
        }
        // 200 stages used to overflow the stack; now refused at the first
        // over-deep reference, in the same bytes it took to build the tower.
        let bytes = sharing_stream(200, 900);
        assert!(matches!(
            load(&bytes, None),
            Err(WireError::Malformed {
                why: "recursion limit",
                ..
            })
        ));
        // Legal sharing still works: 400 + 400 is 800 deep.
        let ok = load(&sharing_stream(2, 400), None).unwrap();
        assert!(height(&ok) <= 1000);
    });
}

/// `DbRowDescriptor::new` compared each column name with every earlier one:
/// quadratic in a client-supplied list. 40,000 columns (a 0.5 MB stream) took
/// nine seconds in a debug build to refuse or accept.
#[test]
fn a_wide_row_descriptor_is_not_quadratic() {
    use eve_marshal::{DbRow, DbRowDescriptor, DbType, DbValue, PyValue, Writer, save};
    let cols = 60_000;
    let names: Vec<String> = (0..cols).map(|i| format!("n{i}")).collect();
    let pairs: Vec<(&str, DbType)> = names.iter().map(|s| (s.as_str(), DbType::I4)).collect();
    let started = Instant::now();
    let d = DbRowDescriptor::from_pairs(&pairs).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    // ...and a duplicate is still refused, wherever it is.
    let mut dup = pairs.clone();
    dup.push(("n0", DbType::I4));
    assert!(DbRowDescriptor::from_pairs(&dup).is_err());
    // the same through the wire
    let row = DbRow::new(d, vec![DbValue::Int(1); cols]).unwrap();
    let mut w = Writer::new();
    w.u8(0x7E);
    w.i32(0);
    save(&PyValue::DbRow(Box::new(row)), &mut w);
    let started = Instant::now();
    let v = load(w.as_slice(), Some(Budget::INBOUND));
    assert!(v.is_ok(), "{v:?}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}
