use matching_engine::AppState;

use crate::{InBoundResponse, inbound};

pub async fn read(buf: Vec<u8>, state: AppState) {
    let inbound = inbound::parse_enter_order(&buf[1..]).unwrap();

    let res = match inbound {
        InBoundResponse::Enter(add_order) => {
            matching_engine::ouch::add_order(
                state,
                add_order.symbol,
                1,
                add_order.side,
                add_order.price,
                add_order.qty,
                add_order.time_in_force,
            )
            .await
        }
        InBoundResponse::Replace(replace_order) => todo!(),
        InBoundResponse::Cancel(cancel_order) => todo!(),
        InBoundResponse::Modify(modify_order) => todo!(),
        InBoundResponse::MassCancel(mass_cancel_order) => todo!(),
        InBoundResponse::DOE(disable_order_entry) => todo!(),
        InBoundResponse::EOE(enable_order_entry) => todo!(),
        InBoundResponse::Query(query_account) => todo!(),
    };
}

pub async fn write() {}
