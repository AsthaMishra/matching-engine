use matching_engine::types::{OrderEvent, Side};

use crate::outbound;

pub struct AddOrder {
    pub user_ref_num: u32,
    pub side: u8,
    pub qty: u32,
    pub symbol: [u8; 8],
    pub price: u64,
    pub time_in_force: u8,
    pub display: char,
    pub capacity: char,
    pub inter_market_sweep_eligibility: char,
    pub cross_type: u8,
    pub ci_ord_id: [u8; 14],
}

impl AddOrder {
    pub fn write(&self, ord_e: OrderEvent, out: &mut Vec<u8>) {
        match ord_e {
            OrderEvent::Accepted {
                id,
                side,
                price,
                qty,
                remaining_qty: _,
            } => {
                let s = match side {
                    Side::Buy => b'B',
                    Side::Sell => b'S',
                    Side::SellShort => b'T',
                    Side::SellShortExempt => b'E',
                };

                let o = outbound::order_accepted(
                    self.user_ref_num,
                    s,
                    qty as u32,
                    self.symbol,
                    price as u64,
                    self.time_in_force,
                    self.time_in_force,
                    id as u64,
                    self.capacity as u8,
                    self.inter_market_sweep_eligibility as u8,
                    self.cross_type,
                    0u8,
                    self.ci_ord_id,
                );
                out.extend_from_slice(&o);
            }
            OrderEvent::Modified {
                id: _,
                side,
                price: _,
                qty,
                remaining_qty: _,
            } => {
                let s = match side {
                    Side::Buy => b'B',
                    Side::Sell => b'S',
                    Side::SellShort => b'T',
                    Side::SellShortExempt => b'E',
                };
                let o = outbound::order_modified(self.user_ref_num, s, qty as u32);
                out.extend_from_slice(&o);
            }
            OrderEvent::Replace {
                id,
                side,
                price,
                qty,
                remaining_qty: _,
            } => {
                let s = match side {
                    Side::Buy => b'B',
                    Side::Sell => b'S',
                    Side::SellShort => b'T',
                    Side::SellShortExempt => b'E',
                };
                let o = outbound::order_replaced(
                    self.user_ref_num,
                    self.user_ref_num,
                    s,
                    qty as u32,
                    self.symbol,
                    price as u64,
                    self.time_in_force,
                    self.time_in_force,
                    id as u64,
                    self.capacity as u8,
                    self.inter_market_sweep_eligibility as u8,
                    self.cross_type,
                    0u8,
                    self.ci_ord_id,
                );
                out.extend_from_slice(&o);
            }
            OrderEvent::Executed(trade) => {
                let o = outbound::order_executed(
                    self.user_ref_num,
                    trade.qty as u32,
                    trade.price as u64,
                    0,
                    trade.id,
                );
                out.extend_from_slice(&o);
            }
            OrderEvent::Canceled {
                id: _,
                qty: _,
                reason,
            } => {
                let o = outbound::order_canceled(self.user_ref_num, self.qty as u32, reason.code());
                out.extend_from_slice(&o);
            }
            OrderEvent::Rejected { id: _, reason } => {
                let o = outbound::order_rejected(
                    self.user_ref_num,
                    reason.code() as u16,
                    self.ci_ord_id,
                );
                out.extend_from_slice(&o);
            }
            OrderEvent::UnknownSymbol { reason } => {
                let o = outbound::order_rejected(
                    self.user_ref_num,
                    reason.code() as u16,
                    self.ci_ord_id,
                );
                out.extend_from_slice(&o);
            }
        }
    }
}

pub struct ReplaceOrder {
    pub org_user_ref_num: u32,
    pub user_ref_num: u32,
    pub qty: u32,
    pub price: u64,
    pub time_in_force: u8,
    pub display: char,
    pub inter_market_sweep_eligibility: char,
    pub ci_ord_id: [u8; 14],
}

impl ReplaceOrder {
    pub fn write(
        &self,
        symbol: [u8; 8],
        capacity: char,
        cross_type: u8,
        ord_e: OrderEvent,
        out: &mut Vec<u8>,
    ) {
        match ord_e {
            OrderEvent::Modified {
                id: _,
                side,
                price: _,
                qty,
                remaining_qty: _,
            } => {
                let s = match side {
                    Side::Buy => b'B',
                    Side::Sell => b'S',
                    Side::SellShort => b'T',
                    Side::SellShortExempt => b'E',
                };
                let o = outbound::order_modified(self.user_ref_num, s, qty as u32);
                out.extend_from_slice(&o);
            }
            OrderEvent::Replace {
                id,
                side,
                price,
                qty,
                remaining_qty: _,
            } => {
                let s = match side {
                    Side::Buy => b'B',
                    Side::Sell => b'S',
                    Side::SellShort => b'T',
                    Side::SellShortExempt => b'E',
                };
                let o = outbound::order_replaced(
                    self.org_user_ref_num,
                    self.user_ref_num,
                    s,
                    qty as u32,
                    symbol,
                    price as u64,
                    self.time_in_force,
                    self.time_in_force,
                    id as u64,
                    capacity as u8,
                    self.inter_market_sweep_eligibility as u8,
                    cross_type,
                    0u8,
                    self.ci_ord_id,
                );
                out.extend_from_slice(&o);
            }
            OrderEvent::Executed(trade) => {
                let o = outbound::order_executed(
                    self.user_ref_num,
                    trade.qty as u32,
                    trade.price as u64,
                    0,
                    trade.id,
                );
                out.extend_from_slice(&o);
            }
            OrderEvent::Canceled {
                id: _,
                qty: _,
                reason,
            } => {
                let o = outbound::order_canceled(self.user_ref_num, self.qty as u32, reason.code());
                out.extend_from_slice(&o);
            }
            OrderEvent::Rejected { id: _, reason } => {
                let o = outbound::order_rejected(
                    self.user_ref_num,
                    reason.code() as u16,
                    self.ci_ord_id,
                );
                out.extend_from_slice(&o);
            }
            OrderEvent::UnknownSymbol { reason } => {
                let o = outbound::order_rejected(
                    self.user_ref_num,
                    reason.code() as u16,
                    self.ci_ord_id,
                );
                out.extend_from_slice(&o);
            }
            OrderEvent::Accepted { .. } => {
                unreachable!("replace order cannot result in accepted event")
            }
        }
    }
}

pub struct CancelOrder {
    pub user_ref_num: u32,
    pub qty: u32,
}

impl CancelOrder {
    pub fn write(&self, ci_ord_id: [u8; 14], ord_e: OrderEvent, out: &mut Vec<u8>) {
        match ord_e {
            OrderEvent::Canceled {
                id: _,
                qty: _,
                reason,
            } => {
                let o = outbound::order_canceled(self.user_ref_num, self.qty as u32, reason.code());
                out.extend_from_slice(&o);
            }
            OrderEvent::Rejected { id: _, reason } => {
                let o =
                    outbound::order_rejected(self.user_ref_num, reason.code() as u16, ci_ord_id);
                out.extend_from_slice(&o);
            }
            OrderEvent::UnknownSymbol { reason } => {
                let o =
                    outbound::order_rejected(self.user_ref_num, reason.code() as u16, ci_ord_id);
                out.extend_from_slice(&o);
            }
            OrderEvent::Accepted { .. } => {
                unreachable!("CancelOrder cannot result in Accepted event")
            }
            OrderEvent::Modified { .. } => {
                unreachable!("CancelOrder cannot result in Modified event")
            }
            OrderEvent::Replace { .. } => {
                unreachable!("CancelOrder cannot result in Replace event")
            }
            OrderEvent::Executed(..) => {
                unreachable!("CancelOrder cannot result in Executed event")
            }
        }
    }
}

pub struct ModifyOrder {
    pub user_ref_num: u32,
    pub side: u8,
    pub qty: u32,
}

impl ModifyOrder {
    pub fn write(&self, ci_ord_id: [u8; 14], ord_e: OrderEvent, out: &mut Vec<u8>) {
        match ord_e {
            OrderEvent::Modified {
                id: _,
                side,
                price: _,
                qty,
                remaining_qty: _,
            } => {
                let s = match side {
                    Side::Buy => b'B',
                    Side::Sell => b'S',
                    Side::SellShort => b'T',
                    Side::SellShortExempt => b'E',
                };
                let o = outbound::order_modified(self.user_ref_num, s, qty as u32);
                out.extend_from_slice(&o);
            }
            OrderEvent::Canceled {
                id: _,
                qty: _,
                reason,
            } => {
                let o = outbound::order_canceled(self.user_ref_num, self.qty as u32, reason.code());
                out.extend_from_slice(&o);
            }
            OrderEvent::Rejected { id: _, reason } => {
                let o =
                    outbound::order_rejected(self.user_ref_num, reason.code() as u16, ci_ord_id);
                out.extend_from_slice(&o);
            }
            OrderEvent::UnknownSymbol { reason } => {
                let o =
                    outbound::order_rejected(self.user_ref_num, reason.code() as u16, ci_ord_id);
                out.extend_from_slice(&o);
            }
            OrderEvent::Accepted { .. } => {
                unreachable!("ModifyOrder cannot result in Accepted event")
            }
            OrderEvent::Replace { .. } => {
                unreachable!("ModifyOrder cannot result in Replace event")
            }
            OrderEvent::Executed(..) => unreachable!("ModifyOrder cannot result in Executed event"),
        }
    }
}

pub struct MassCancelOrder {
    pub user_ref_num: u32,
    pub firm: [u8; 8],
    pub symbol: [u8; 8],
}

pub struct DisableOrderEntry {
    pub user_ref_num: u32,
    pub firm: [u8; 8],
}

pub struct EnableOrderEntry {
    pub user_ref_num: u32,
    pub firm: [u8; 8],
}

pub struct QueryAccount {
    pub user_ref_idx: Option<u8>,
}

pub enum InBoundResponse {
    Enter(AddOrder),
    Replace(ReplaceOrder),
    Cancel(CancelOrder),
    Modify(ModifyOrder),
    MassCancel(MassCancelOrder),
    DOE(DisableOrderEntry),
    EOE(EnableOrderEntry),
    Query(QueryAccount),
}
