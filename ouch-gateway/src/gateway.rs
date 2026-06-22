use matching_engine::{
    match_order_into, matching,
    order_book::OrderBook,
    types::{CancelRejectReason, CommandType, Order, OrderEvent, OrderType, Side},
};

use crate::{InBoundResponse, OrderHandle, Session, inbound};

// `out` (response bytes) and `ev_buf` (engine events) are caller-owned and
// reused across messages - both are cleared here / by match_order_into, so the
// hot path does zero per-message allocation in steady state.
pub fn read(
    buf: Vec<u8>,
    book: &mut OrderBook,
    sess: &mut Session,
    out: &mut Vec<u8>,
    ev_buf: &mut Vec<OrderEvent>,
) -> u64 {
    out.clear();
    let inbound = inbound::parse_enter_order(&buf[1..]).unwrap();
    let mut eng_ns: u64 = 0;

    match inbound {
        InBoundResponse::Enter(a_o) => {
            let s: Side = match a_o.side {
                b'B' => Side::Buy,
                b'S' => Side::Sell,
                b'T' => Side::SellShort,
                b'E' => Side::SellShortExempt,
                _ => Side::Buy,
            };

            let ord_type = match a_o.time_in_force {
                b'0' => OrderType::Limit,
                b'3' => OrderType::IOC,
                b'5' => OrderType::FOK,
                b'6' => OrderType::Market,
                _ => OrderType::Limit,
            };

            let order = Order::new(
                book.allocate_id(),
                sess.session_id,
                s,
                ord_type,
                a_o.price as i64,
                a_o.qty as u64,
                a_o.qty as u64,
                // now_nanos(),
            );

            let eng_t = std::time::Instant::now();
            match_order_into(book, order, CommandType::Add, ev_buf);
            eng_ns = eng_t.elapsed().as_nanos() as u64;

            for ev in ev_buf.drain(..) {
                if let OrderEvent::Accepted { id, .. } = &ev {
                    sess.map.insert(
                        a_o.user_ref_num,
                        OrderHandle {
                            order_id: *id as usize,
                            symbol: a_o.symbol,
                            capacity: a_o.capacity,
                            cross_type: a_o.cross_type,
                            ci_ord_id: a_o.ci_ord_id,
                        },
                    );
                }
                a_o.write(ev, out)
            }
        }
        InBoundResponse::Replace(r_o) => {
            let Some(ord_h) = sess.map.get(&r_o.user_ref_num) else {
                return eng_ns;
            };

            let events =
                matching::replace_order(book, ord_h.order_id, r_o.price as i64, r_o.qty as u64);

            match events {
                Ok(evs) => {
                    for ev in evs {
                        r_o.write(ord_h.symbol, ord_h.capacity, ord_h.cross_type, ev, out);
                    }
                }
                Err(_) => {
                    r_o.write(
                        [0u8; 8],
                        '0',
                        0,
                        OrderEvent::Rejected {
                            id: ord_h.order_id,
                            reason: CancelRejectReason::OrderNotActive,
                        },
                        out,
                    );
                }
            }
        }
        InBoundResponse::Cancel(c_o) => {
            let Some(ord_h) = sess.map.get(&c_o.user_ref_num) else {
                return eng_ns;
            };

            let ev_r = book.cancel_order(ord_h.order_id);
            match ev_r {
                Ok(ev) => {
                    c_o.write(ord_h.ci_ord_id, ev, out);
                }
                Err(_) => {
                    c_o.write(
                        [0u8; 14],
                        OrderEvent::Rejected {
                            id: ord_h.order_id,
                            reason: CancelRejectReason::OrderNotActive,
                        },
                        out,
                    );
                }
            }
        }
        InBoundResponse::Modify(m_o) => {
            let Some(ord_h) = sess.map.get(&m_o.user_ref_num) else {
                return eng_ns;
            };

            let eng_t = std::time::Instant::now();
            let events = matching::modify_order(book, ord_h.order_id, m_o.qty as u64);
            eng_ns = eng_t.elapsed().as_nanos() as u64;

            match events {
                Ok(evs) => {
                    for ev in evs {
                        m_o.write(ord_h.ci_ord_id, ev, out);
                    }
                }
                Err(_) => {
                    m_o.write(
                        [0u8; 14],
                        OrderEvent::Rejected {
                            id: ord_h.order_id,
                            reason: CancelRejectReason::OrderNotActive,
                        },
                        out,
                    );
                }
            }
        }
        InBoundResponse::MassCancel(_mass_cancel_order) => todo!(),
        InBoundResponse::DOE(_disable_order_entry) => todo!(),
        InBoundResponse::EOE(_enable_order_entry) => todo!(),
        InBoundResponse::Query(_query_account) => todo!(),
    };

    eng_ns
}
