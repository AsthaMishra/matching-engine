use crate::{
    AddOrder, CancelOrder, DisableOrderEntry, EnableOrderEntry, InBoundResponse, MassCancelOrder,
    ModifyOrder, QueryAccount, ReplaceOrder,
};

pub fn parse_enter_order(buf: &[u8]) -> Result<InBoundResponse, &'static str> {
    match buf[0] {
        //order place
        b'O' => {
            let user_ref_num = u32::from_be_bytes(buf[1..5].try_into().unwrap());

            // B= buy
            // S = sell
            // T = sell short
            // E = sell short exempt
            let side = buf[5];
            let qty = u32::from_be_bytes(buf[6..10].try_into().unwrap());
            let symbol = buf[10..18].try_into().unwrap();
            let price = u64::from_be_bytes(buf[18..26].try_into().unwrap());

            // "How long should this order live?"
            //0 = Day	Cancel at end of trading day
            //3 = IOC	Immediate-or-Cancel — fill what you can right now, cancel the rest instantly
            //5 = GTX	Good-till-extended — survives into after-hours session
            //6 = GTT	Good-till-time — cancel at a specific time (needs ExpireTime tag)
            //E = After hours	Only active in after-hours
            let time_in_force = buf[26];

            // "Should other market participants see this order on the book?"
            // Y	Visible — shows up in public order book (Level 2 data)
            // N	Hidden — resting on the book but invisible to others
            // A	Attributable — visible AND your firm's name is shown
            let display = buf[27] as char;

            // "Are you trading for yourself or on behalf of a client?"
            // A = Agency	You're a broker acting for a client
            // P = Principal	You're trading your own firm's money
            // R = Riskless	A simultaneous buy and sell, risk-free to you
            // O = Other	Doesn't fit the above
            let capacity = buf[28] as char;

            // "Can this order sweep across multiple exchanges?"
            // N	Normal continuous trading (what you're building)
            // O	Opening cross — NASDAQ runs a special auction at 9:30am
            // C	Closing cross — special auction at 4:00pm
            // H	Halt/IPO cross — when a stock resumes trading after a halt
            // S	Supplemental
            let inter_market_sweep_eligibility = buf[29] as char;

            // "Which special auction/cross mechanism should this order participate in?"
            // N	Normal continuous trading (what you're building)
            // O	Opening cross — NASDAQ runs a special auction at 9:30am
            // C	Closing cross — special auction at 4:00pm
            // H	Halt/IPO cross — when a stock resumes trading after a halt
            // S	Supplemental
            let cross_type = buf[30];

            // let ci_ord_id = str::from_utf8(buf[31..45].try_into().unwrap()).unwrap();
            let ci_ord_id = buf[31..45].try_into().unwrap();
            let appendage_length = u16::from_be_bytes(buf[45..47].try_into().unwrap());
            let tag_value_length = buf[47] as usize;
            let tag = buf[48];
            let value = str::from_utf8(&buf[49..49 + tag_value_length - 1]);

            Ok(InBoundResponse::Enter(AddOrder {
                user_ref_num,
                side,
                qty,
                symbol,
                price,
                time_in_force,
                display,
                capacity,
                inter_market_sweep_eligibility,
                cross_type,
                ci_ord_id,
            }))
        }
        //order replace
        b'U' => {
            let org_user_ref_num = u32::from_be_bytes(buf[1..5].try_into().unwrap());
            let user_ref_num = u32::from_be_bytes(buf[5..9].try_into().unwrap());
            let qty = u32::from_be_bytes(buf[9..13].try_into().unwrap());
            let price = u64::from_be_bytes(buf[13..21].try_into().unwrap());
            let time_in_force = buf[21];
            let display = buf[22] as char;
            let inter_market_sweep_eligibility = buf[23] as char;

            let ci_ord_id = buf[24..38].try_into().unwrap();
            let appendage_length = u16::from_be_bytes(buf[38..40].try_into().unwrap());
            let tag_value_length = buf[40] as usize;
            let tag = buf[41];
            let value = str::from_utf8(&buf[42..42 + tag_value_length - 1]);

            Ok(InBoundResponse::Replace(ReplaceOrder {
                org_user_ref_num,
                user_ref_num,
                qty,
                price,
                time_in_force,
                display,
                inter_market_sweep_eligibility,
                ci_ord_id,
            }))
        }
        // order cancel
        b'X' => {
            let user_ref_num = u32::from_be_bytes(buf[1..5].try_into().unwrap());
            let qty = u32::from_be_bytes(buf[5..9].try_into().unwrap());

            let appendage_length = u16::from_be_bytes(buf[9..11].try_into().unwrap());
            let tag_value_length = buf[11] as usize;
            let tag = buf[12];
            let value = str::from_utf8(&buf[13..13 + tag_value_length - 1]);
            Ok(InBoundResponse::Cancel(CancelOrder { user_ref_num, qty }))
        }
        // modify order
        b'M' => {
            let user_ref_num = u32::from_be_bytes(buf[1..5].try_into().unwrap());
            let side = buf[5];
            let qty = u32::from_be_bytes(buf[6..10].try_into().unwrap());

            let appendage_length = u16::from_be_bytes(buf[10..12].try_into().unwrap());
            let tag_value_length = buf[12] as usize;
            let tag = buf[13];
            let value = str::from_utf8(&buf[14..14 + tag_value_length - 1]);
            Ok(InBoundResponse::Modify(ModifyOrder {
                user_ref_num,
                side,
                qty,
            }))
        }
        // mass cancel request
        b'C' => {
            let user_ref_num = u32::from_be_bytes(buf[1..5].try_into().unwrap());
            let firm = buf[5..9].try_into().unwrap();
            let symbol = buf[9..17].try_into().unwrap();

            let appendage_length = u16::from_be_bytes(buf[17..19].try_into().unwrap());
            let tag_value_length = buf[19] as usize;
            let tag = buf[20];
            let value = str::from_utf8(&buf[21..21 + tag_value_length - 1]);
            Ok(InBoundResponse::MassCancel(MassCancelOrder {
                user_ref_num,
                firm,
                symbol,
            }))
        }
        //disable order entry
        b'D' => {
            let user_ref_num = u32::from_be_bytes(buf[1..5].try_into().unwrap());
            let firm = buf[5..9].try_into().unwrap();
            let appendage_length = u16::from_be_bytes(buf[9..11].try_into().unwrap());
            let tag_value_length = buf[11] as usize;
            let tag = buf[12];
            let value = str::from_utf8(&buf[13..13 + tag_value_length - 1]);

            Ok(InBoundResponse::DOE(DisableOrderEntry {
                user_ref_num,
                firm,
            }))
        }
        // enable order entry
        b'E' => {
            let user_ref_num = u32::from_be_bytes(buf[1..5].try_into().unwrap());
            let firm = buf[5..9].try_into().unwrap();
            let appendage_length = u16::from_be_bytes(buf[9..11].try_into().unwrap());
            let tag_value_length = buf[11] as usize;
            let tag = buf[12];
            let value = str::from_utf8(&buf[13..13 + tag_value_length - 1]);

            Ok(InBoundResponse::EOE(EnableOrderEntry {
                user_ref_num,
                firm,
            }))
        }
        // account query
        b'Q' => {
            let appendage_length = u16::from_be_bytes(buf[1..3].try_into().unwrap());
            let tag_value_length = buf[3] as usize;
            let tag = buf[4];
            let value = str::from_utf8(&buf[5..5 + tag_value_length - 1]);

            let user_ref_idx = if appendage_length > 0 {
                Some(buf[5]) // buf[3]=tv_length, buf[4]=tag(28), buf[5]=value
            } else {
                None
            };
            Ok(InBoundResponse::Query(QueryAccount { user_ref_idx }))
        }
        _ => Err("unknown message type"),
    }
}

#[test]
pub fn test_eo() {}
