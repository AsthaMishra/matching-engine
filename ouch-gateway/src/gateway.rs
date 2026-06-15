use matching_engine::{
    AppState,
    types::{OrderEvent, OrderType, Side},
};

use crate::{AddOrder, InBoundResponse, Session, inbound, outbound};

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

            let a_o_r = matching_engine::ouch::add_order(
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

            let data = a_o_r.data;

            for r in data {
                res.extend_from_slice(&write_ao(&add_order, r));
            }
        }
        InBoundResponse::Replace(replace_order) => {
            // matching_engine::ouch::update_order(state, replace_order.symbol, order_id, price, qty)
            //     .await
        }
        InBoundResponse::Cancel(cancel_order) => todo!(),
        InBoundResponse::Modify(modify_order) => todo!(),
        InBoundResponse::MassCancel(mass_cancel_order) => todo!(),
        InBoundResponse::DOE(disable_order_entry) => todo!(),
        InBoundResponse::EOE(enable_order_entry) => todo!(),
        InBoundResponse::Query(query_account) => todo!(),
    };

    // let res_data = res.data;

    // for ord_e in res_data {}

    res
}

pub fn write_ao(ord: &AddOrder, ord_e: OrderEvent) -> Vec<u8> {
    let mut res: Vec<u8> = vec![];

    match ord_e {
        OrderEvent::Accepted {
            id,
            side,
            price,
            qty,
            remaining_qty,
        } => {
            let s = match side {
                Side::Buy => b'B',
                Side::Sell => b'S',
                Side::Sell_Short => b'T',
                Side::Sell_Short_Exempt => b'E',
            };

            let o = outbound::order_accepted(
                ord.user_ref_num,
                s,
                qty as u32,
                ord.symbol,
                price as u64,
                ord.time_in_force,
                ord.time_in_force,
                id as u64,
                ord.capacity as u8,
                ord.inter_market_sweep_eligibility as u8,
                ord.cross_type,
                0u8,
                ord.ci_ord_id.as_str(),
            );
            res.extend_from_slice(&o);
        }
        OrderEvent::Modified {
            id,
            side,
            price,
            qty,
            remaining_qty,
        } => {
            let s = match side {
                Side::Buy => b'B',
                Side::Sell => b'S',
                Side::Sell_Short => b'T',
                Side::Sell_Short_Exempt => b'E',
            };
            let o = outbound::order_modified(ord.user_ref_num, s, qty as u32);
            res.extend_from_slice(&o);
        }
        OrderEvent::Replace {
            id,
            side,
            price,
            qty,
            remaining_qty,
        } => {
            let s = match side {
                Side::Buy => b'B',
                Side::Sell => b'S',
                Side::Sell_Short => b'T',
                Side::Sell_Short_Exempt => b'E',
            };
            let o = outbound::order_replaced(
                ord.user_ref_num,
                ord.user_ref_num,
                s,
                qty as u32,
                ord.symbol,
                price as u64,
                ord.time_in_force,
                ord.time_in_force,
                id as u64,
                ord.capacity as u8,
                ord.inter_market_sweep_eligibility as u8,
                ord.cross_type,
                0u8,
                ord.ci_ord_id.as_str(),
            );
            res.extend_from_slice(&o);
        }
        OrderEvent::Executed(trade) => {
            let o = outbound::order_executed(
                ord.user_ref_num,
                trade.qty as u32,
                trade.price as u64,
                0,
                trade.id,
            );
            res.extend_from_slice(&o);
        }
        OrderEvent::Canceled { id, qty, reason } => {
            let o = outbound::order_canceled(ord.user_ref_num, ord.qty as u32, reason.code());
            res.extend_from_slice(&o);
        }
        OrderEvent::Rejected { id, reason } => {
            let o =
                outbound::order_rejected(ord.user_ref_num, reason.code() as u16, &ord.ci_ord_id);
            res.extend_from_slice(&o);
        }
        OrderEvent::UnknownSymbol { reason } => {
            let o =
                outbound::order_rejected(ord.user_ref_num, reason.code() as u16, &ord.ci_ord_id);
            res.extend_from_slice(&o);
        }
    }

    res
}
