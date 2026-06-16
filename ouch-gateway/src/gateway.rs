use matching_engine::{
    AppState,
    types::{OrderEvent, OrderType, Side},
};

use crate::{InBoundResponse, OrderHandle, Session, inbound};

pub async fn read(buf: Vec<u8>, state: AppState, sess: &mut Session) -> Vec<u8> {
    let inbound = inbound::parse_enter_order(&buf[1..]).unwrap();
    let mut res: Vec<u8> = vec![];

    match inbound {
        InBoundResponse::Enter(add_order) => {
            let s: Side = match add_order.side {
                b'B' => Side::Buy,
                b'S' => Side::Sell,
                b'T' => Side::Sell_Short,
                b'E' => Side::Sell_Short_Exempt,
                _ => Side::Buy,
            };

            let ord_type = match add_order.time_in_force {
                b'0' => OrderType::Limit,
                b'3' => OrderType::IOC,
                b'5' => OrderType::FOK,
                b'6' => OrderType::Market,
                _ => OrderType::Limit,
            };

            let Some(symbol_id) = state
                .symbol_registery
                .read()
                .unwrap()
                .look_up(add_order.symbol)
            else {
                return res;
            };

            let Some(sender) = state.senders.get(&symbol_id).cloned() else {
                unreachable!(
                    "symbol_id {symbol_id} in registry but no sender — registry/senders desynced"
                );
            };

            let o_res = matching_engine::ouch::add_order(
                state,
                add_order.symbol,
                1,
                s,
                add_order.price,
                add_order.qty,
                ord_type,
                add_order.time_in_force,
            )
            .await;

            let data = o_res.data;

            for r in data {
                if let OrderEvent::Accepted { id, .. } = &r {
                    sess.map.insert(
                        add_order.user_ref_num,
                        OrderHandle {
                            sender: sender.clone(),
                            order_id: *id as usize,
                            symbol: add_order.symbol,
                            capacity: add_order.capacity,
                            cross_type: add_order.cross_type,
                            ci_ord_id: add_order.ci_ord_id,
                        },
                    );
                }
                let buf = add_order.write(r);
                res.extend_from_slice(&buf);
            }
        }
        InBoundResponse::Replace(replace_order) => {
            let Some(ord_h) = sess.map.get(&replace_order.user_ref_num) else {
                return res;
            };
            let o_res = matching_engine::ouch::replace_order(
                state,
                ord_h.symbol,
                ord_h.order_id.try_into().unwrap(),
                replace_order.price,
                replace_order.qty,
            )
            .await;

            let data = o_res.data;

            for r in data {
                let buf = replace_order.write(ord_h.symbol, ord_h.capacity, ord_h.cross_type, r);
                res.extend_from_slice(&buf);
            }
        }
        InBoundResponse::Cancel(cancel_order) => {
            let Some(ord_h) = sess.map.get(&cancel_order.user_ref_num) else {
                return res;
            };
            let o_res = matching_engine::ouch::cancel_order(
                state,
                ord_h.symbol,
                ord_h.order_id.try_into().unwrap(),
            )
            .await;

            let r = o_res.data;

            let buf = cancel_order.write(ord_h.ci_ord_id, r);
            res.extend_from_slice(&buf);
        }
        InBoundResponse::Modify(modify_order) => {
            let Some(ord_h) = sess.map.get(&modify_order.user_ref_num) else {
                return res;
            };
            let o_res = matching_engine::ouch::modify_order(
                state,
                ord_h.symbol,
                ord_h.order_id.try_into().unwrap(),
                modify_order.qty,
            )
            .await;

            for r in o_res.data {
                let buf = modify_order.write(ord_h.ci_ord_id, r);
                res.extend_from_slice(&buf);
            }
        }
        InBoundResponse::MassCancel(mass_cancel_order) => todo!(),
        InBoundResponse::DOE(disable_order_entry) => todo!(),
        InBoundResponse::EOE(enable_order_entry) => todo!(),
        InBoundResponse::Query(query_account) => todo!(),
    };

    res
}
