use rust_provider_lib::state_machine::StateMachine;
use rust_provider_lib::core_pb::{
    ProviderMsg, RegisterRet, StartGameArgs, QueryStateArgs, UserOperationArgs,
    ErrorNumber
};
use rust_provider_lib::util;
use crate::splendor_pb::{UserOperation, StartGameSettings, Take, Purchase, Reserve,
    user_operation::Operation, ResourceType
};
use crate::state::{GlobalState};
use crate::msg_builder as mb;
use rust_provider_lib::msg_builder as pmb;

pub struct SplendorStateMachine {
    state: GlobalState
}

impl StateMachine for SplendorStateMachine {
    fn transition_register_ret(&mut self, msg: RegisterRet) -> Vec<ProviderMsg> {
        assert_eq!(msg.err, ErrorNumber::Ok as i32);
        vec![]
    }

    fn transition_start_game_args(&mut self, msg: StartGameArgs) -> Vec<ProviderMsg> {
        let game_settings = util::unpack::<StartGameSettings>(msg.custom.unwrap());
        let game_start = self.state.new_game(msg.room_id, msg.user_id, game_settings);

        vec![pmb::notify_msg_args(
            0, ErrorNumber::Ok, msg.room_id, 0, mb::game_start(game_start))]
    }

    fn transition_query_state_args(&mut self, _msg: QueryStateArgs) -> Vec<ProviderMsg> {
        vec![]
    }

    fn transition_user_operation_args(&mut self, msg: UserOperationArgs) -> Vec<ProviderMsg> {
        self.state.cur_roomid = msg.room_id;
        self.state.cur_userid = msg.user_id as i32;
        self.transition_user_operation(util::unpack::<UserOperation>(msg.custom.unwrap()))
    }
}

impl SplendorStateMachine {
    pub fn new() -> SplendorStateMachine {
        SplendorStateMachine{ state: GlobalState::new() }
    }

    fn transition_user_operation(&mut self, msg: UserOperation) -> Vec<ProviderMsg> {
        let mut msgs = match msg.clone().operation.unwrap() {
            Operation::Take(msg) => self.transition_take(msg),
            Operation::Purchase(msg) => self.transition_purchase(msg),
            Operation::Reserve(msg) => self.transition_reserve(msg),
        };

        let roomid = self.state.cur_roomid;
        let userid = self.state.cur_userid as u32;
        // broadcast the user operation to all other players
        msgs.push(pmb::notify_msg_args(
            0, ErrorNumber::Ok, roomid, -(userid as i32), mb::user_op(msg)));
        msgs
    }

    fn transition_take(&mut self, msg: Take) -> Vec<ProviderMsg> {
        let roomid = self.state.cur_roomid;
        let userid = self.state.cur_userid as u32;

        let game_state = self.state.roomid_to_game_state.get_mut(&roomid).unwrap();
        let player_state = game_state.userid_to_player_state.get_mut(&userid).unwrap();

        // step 1. transfer resources
        for resource_type in msg.resources.iter() {
            game_state.board_state.take_resource(*resource_type);
            player_state.take_resource(*resource_type);
        }

        vec![pmb::notify_msg_args(
            0, ErrorNumber::Ok, roomid, 0, mb::game_state(game_state.get_pub_state()))]
    }

    fn transition_purchase(&mut self, msg: Purchase) -> Vec<ProviderMsg> {
        let roomid = self.state.cur_roomid;
        let userid = self.state.cur_userid as u32;

        let game_state = self.state.roomid_to_game_state.get_mut(&roomid).unwrap();
        let board_state = &mut game_state.board_state;
        let player_state = game_state.userid_to_player_state.get_mut(&userid).unwrap();

        // step 1. transfer dev card
        let dev_card = if msg.development_level >= 0 {
            // purchase the dev card on board
            board_state.dev_cards[msg.development_level as usize].take(msg.index)
        }
        else {
            // purchase the dev card reserved in advance
            player_state.reserved_cards.remove(msg.index as usize)
        };
        player_state.dev_cards.push(dev_card.clone());
        player_state.points += dev_card.points;

        // step 2. transfer resources
        player_state.spend_resource(dev_card.clone().price.unwrap());
        board_state.return_resource(dev_card.price.unwrap());

        // step 3. transfer noble if any
        let res_map = player_state.get_pub_state().development.unwrap();
        let mut nobles_will_visit = vec![];
        for (i, noble) in board_state.nobles_on_board.iter().enumerate() {
            let mut will_visit = true;
            for entry in noble.capital.as_ref().unwrap().entries.iter() {
                if entry.number > res_map.entries[entry.resource_type as usize].number {
                    will_visit = false;
                    break;
                }
            }
            if will_visit {
                nobles_will_visit.push(i);
            }
        }
        for i in nobles_will_visit.iter().rev() {
            let noble =  board_state.nobles_on_board.remove(*i);
            player_state.nobles.push(noble.clone());
            player_state.points += noble.points;
        }

        vec![pmb::notify_msg_args(
            0, ErrorNumber::Ok, roomid, 0, mb::game_state(game_state.get_pub_state()))]
    }

    fn transition_reserve(&mut self, msg: Reserve) -> Vec<ProviderMsg> {
        let roomid = self.state.cur_roomid;
        let userid = self.state.cur_userid as u32;

        let game_state = self.state.roomid_to_game_state.get_mut(&roomid).unwrap();
        let board_state = &mut game_state.board_state;
        let player_state = game_state.userid_to_player_state.get_mut(&userid).unwrap();
        let mut msgs = vec![];

        // step 1. transfer dev card
        let dev_card = if msg.index >= 0 {
            // reserve the dev card on board
            board_state.dev_cards[msg.development_level as usize].take(msg.index)
        }
        else {
            // reserve the dev card on the top of deck
            assert!(!board_state.dev_cards[msg.development_level as usize].deck.is_empty());
            let card = board_state.dev_cards[msg.development_level as usize].deck.pop().unwrap();
            // need to tell only that player what the dev card is
            msgs.push(pmb::notify_msg_args(
                0, ErrorNumber::Ok, roomid, userid as i32, mb::reserve_rsp(card.clone())));
            card
        };
        player_state.reserved_cards.push(dev_card);

        // step 2. get the gold if any
        if board_state.resources_on_board.entries[ResourceType::Gold as usize].number > 0 {
            board_state.take_resource(ResourceType::Gold as i32);
            player_state.take_resource(ResourceType::Gold as i32);
        }

        msgs.push(pmb::notify_msg_args(
            0, ErrorNumber::Ok, roomid, 0, mb::game_state(game_state.get_pub_state())));
        msgs
    }
}