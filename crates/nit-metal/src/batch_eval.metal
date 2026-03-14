#include <metal_stdlib>

using namespace metal;

#ifndef CA_MAX_WINDOW
#define CA_MAX_WINDOW 1024u
#endif
#ifndef TM_MAX_WIDTH
#define TM_MAX_WIDTH 1024u
#endif
#ifndef FSM_MAX_STATES
#define FSM_MAX_STATES 4u
#endif

struct MatchPair {
    uint a_idx;
    uint b_idx;
};

struct ScorePair {
    long a_total;
    long b_total;
};

struct TmHaltingPair {
    uint a_all_halted;
    uint b_all_halted;
};

struct EvalParams {
    uint rounds;
    uint pair_count;
    int cc_a;
    int cc_b;
    int cd_a;
    int cd_b;
    int dc_a;
    int dc_b;
    int dd_a;
    int dd_b;
    int timeout_lose;
    int timeout_win;
};

struct FsmParams {
    uint states;
    uint alphabet;
};

struct CaParams {
    uint symbols;
    uint two_r;
    uint steps;
    uint rule_table_len;
};

struct TmParams {
    uint states;
    uint symbols;
    uint blank;
    uint max_steps;
    uint transitions_per_strategy;
};

struct TmTransition {
    uint write;
    uint move_dir;
    uint next;
    uint _pad;
};

inline long2 payoff_for_actions(uint a_action, uint b_action, constant EvalParams& params) {
    if (a_action == 0u && b_action == 0u) {
        return long2(params.cc_a, params.cc_b);
    }
    if (a_action == 0u && b_action == 1u) {
        return long2(params.cd_a, params.cd_b);
    }
    if (a_action == 1u && b_action == 0u) {
        return long2(params.dc_a, params.dc_b);
    }
    return long2(params.dd_a, params.dd_b);
}

inline long2 payoff_with_timeouts(
    uint a_action,
    uint b_action,
    bool a_halted,
    bool b_halted,
    constant EvalParams& params
) {
    if (a_halted && b_halted) {
        return payoff_for_actions(a_action, b_action, params);
    }
    if (!a_halted && !b_halted) {
        return long2(params.timeout_lose, params.timeout_lose);
    }
    if (!a_halted) {
        return long2(params.timeout_lose, params.timeout_win);
    }
    return long2(params.timeout_win, params.timeout_lose);
}

inline void push_bit(thread uint* history_bits, thread uint& history_len, uint bit) {
    if (history_len < CA_MAX_WINDOW) {
        history_bits[history_len] = bit;
        history_len += 1u;
        return;
    }
    if (CA_MAX_WINDOW == 0u) {
        return;
    }
    for (uint idx = 1u; idx < history_len; idx++) {
        history_bits[idx - 1u] = history_bits[idx];
    }
    history_bits[history_len - 1u] = bit;
}

inline uint ca_action_for_strategy(
    device const uint* rule_tables,
    constant CaParams& params,
    uint strategy_idx,
    const thread uint* history_bits,
    uint history_len
) {
    if (history_len == 0u) {
        return 0u;
    }
    thread uint row[CA_MAX_WINDOW];
    thread uint next_row[CA_MAX_WINDOW];
    uint row_len = history_len;
    for (uint idx = 0u; idx < history_len; idx++) {
        row[idx] = history_bits[idx];
    }
    const uint neighborhood = params.two_r + 1u;
    const uint table_base = strategy_idx * params.rule_table_len;
    for (uint step = 0u; step < params.steps; step++) {
        if (neighborhood == 0u || row_len <= params.two_r) {
            break;
        }
        const uint next_len = row_len - params.two_r;
        if (next_len == 0u) {
            break;
        }
        for (uint start = 0u; start < next_len; start++) {
            uint table_idx = 0u;
            for (uint offset = 0u; offset < neighborhood; offset++) {
                table_idx = table_idx * params.symbols + row[start + offset];
            }
            next_row[start] = rule_tables[table_base + table_idx];
        }
        for (uint idx = 0u; idx < next_len; idx++) {
            row[idx] = next_row[idx];
        }
        row_len = next_len;
    }
    return row[row_len - 1u] == 0u ? 0u : 1u;
}

inline void tm_trim_redundant_high_zeros(thread uchar* digits, thread uint& len) {
    while (len > 1u && digits[len - 1u] == 0u) {
        len -= 1u;
    }
}

inline void tm_trim_high_zeros_with_prefix(thread uchar* digits, thread uint& len, uint width) {
    while (len > width) {
        len -= 1u;
    }
    while (len > 1u && digits[len - 1u] == 0u) {
        if (len == width) {
            break;
        }
        len -= 1u;
    }
}

inline void tm_mul_add(
    thread uchar* digits,
    thread uint& len,
    thread uint& prefix_nonzero,
    uint width,
    uint base,
    uint mul,
    uint add
) {
    uint carry = add;
    for (uint idx = 0u; idx < len; idx++) {
        const uint value = uint(digits[idx]) * mul + carry;
        digits[idx] = (uchar)(value % base);
        carry = value / base;
    }
    while (carry > 0u) {
        if (len < width) {
            digits[len] = (uchar)(carry % base);
            len += 1u;
            carry /= base;
        } else {
            prefix_nonzero = 1u;
            break;
        }
    }
    while (len > width) {
        const uint popped = uint(digits[len - 1u]);
        len -= 1u;
        if (popped != 0u) {
            prefix_nonzero = 1u;
        }
    }
    if (prefix_nonzero != 0u) {
        tm_trim_high_zeros_with_prefix(digits, len, width);
    } else {
        tm_trim_redundant_high_zeros(digits, len);
    }
}

inline void tm_push_round(
    thread uchar* digits,
    thread uint& len,
    thread uint& prefix_nonzero,
    uint width,
    uint pair_digit,
    uint base
) {
    tm_mul_add(digits, len, prefix_nonzero, width, base, 4u, pair_digit);
}

inline void tm_input_digits(
    const thread uchar* digits_le,
    uint digits_len,
    uint prefix_nonzero,
    thread uchar* input_digits,
    thread uint& input_len
) {
    input_len = digits_len;
    for (uint idx = 0u; idx < digits_len; idx++) {
        input_digits[idx] = digits_le[digits_len - 1u - idx];
    }
    if (prefix_nonzero == 0u) {
        uint start = 0u;
        while (start + 1u < input_len && input_digits[start] == 0u) {
            start += 1u;
        }
        if (start > 0u) {
            for (uint idx = start; idx < input_len; idx++) {
                input_digits[idx - start] = input_digits[idx];
            }
            input_len -= start;
        }
    }
    if (input_len == 0u) {
        input_digits[0] = 0u;
        input_len = 1u;
    }
}

inline uint tm_action_for_strategy(
    device const TmTransition* transitions,
    device const uint* start_states,
    constant TmParams& params,
    uint strategy_idx,
    const thread uchar* input_digits,
    uint input_len,
    thread bool& halted
) {
    thread uchar tape[TM_MAX_WIDTH];
    for (uint idx = 0u; idx < input_len; idx++) {
        tape[idx] = input_digits[idx];
    }
    uint tape_len = input_len;
    if (tape_len == 0u) {
        tape[0] = 0u;
        tape_len = 1u;
    }
    uint head = tape_len - 1u;
    uint state = start_states[strategy_idx];
    halted = false;
    if (params.max_steps == 0u || state == 0u) {
        return 1u;
    }
    const uint base = strategy_idx * params.transitions_per_strategy;
    for (uint step = 0u; step < params.max_steps; step++) {
        const uint read = head < tape_len ? uint(tape[head]) : params.blank;
        const TmTransition trans = transitions[base + (state - 1u) * params.symbols + read];
        tape[head] = (uchar)trans.write;
        if (trans.move_dir == 1u && head + 1u == tape_len) {
            halted = true;
            return tape[tape_len - 1u] == (uchar)0 ? 0u : 1u;
        }
        if (trans.move_dir == 0u) {
            if (head > 0u) {
                head -= 1u;
            } else if (tape_len < TM_MAX_WIDTH) {
                for (uint idx = tape_len; idx > 0u; idx--) {
                    tape[idx] = tape[idx - 1u];
                }
                tape[0] = (uchar)params.blank;
                tape_len += 1u;
            }
        } else if (trans.move_dir == 1u) {
            if (head + 1u < tape_len) {
                head += 1u;
            }
        }
        state = trans.next;
        if (state == 0u) {
            return 1u;
        }
    }
    return 1u;
}

kernel void fsm_batch(
    device const MatchPair* pairs [[buffer(0)]],
    device const uint* starts [[buffer(1)]],
    device const uint* outputs [[buffer(2)]],
    device const uint* transitions [[buffer(3)]],
    device ScorePair* scores [[buffer(4)]],
    constant EvalParams& eval_params [[buffer(5)]],
    constant FsmParams& fsm_params [[buffer(6)]],
    uint gid [[thread_position_in_grid]]
) {
    if (gid >= eval_params.pair_count) {
        return;
    }
    const auto pair = pairs[gid];
    uint a_state = starts[pair.a_idx];
    uint b_state = starts[pair.b_idx];
    long a_total = 0;
    long b_total = 0;

    // Cycle detection: FSM combined state space is states*states, so a cycle
    // must appear within that many rounds.  When detected, skip ahead in O(1).
    const uint max_combined = FSM_MAX_STATES * FSM_MAX_STATES;
    thread uint cycle_round[FSM_MAX_STATES * FSM_MAX_STATES];
    thread long cycle_a[FSM_MAX_STATES * FSM_MAX_STATES];
    thread long cycle_b[FSM_MAX_STATES * FSM_MAX_STATES];
    for (uint i = 0u; i < max_combined; i++) {
        cycle_round[i] = 0xFFFFFFFFu;
    }

    for (uint round = 0u; round < eval_params.rounds; round++) {
        const uint combined = a_state * fsm_params.states + b_state;
        if (combined < max_combined) {
            if (cycle_round[combined] != 0xFFFFFFFFu) {
                const uint cycle_len = round - cycle_round[combined];
                if (cycle_len > 0u) {
                    const long ca = a_total - cycle_a[combined];
                    const long cb = b_total - cycle_b[combined];
                    const uint remaining = eval_params.rounds - round;
                    const uint full_cycles = remaining / cycle_len;
                    a_total += long(full_cycles) * ca;
                    b_total += long(full_cycles) * cb;
                    const uint leftover = remaining - full_cycles * cycle_len;
                    for (uint r = 0u; r < leftover; r++) {
                        const uint aa = outputs[pair.a_idx * fsm_params.states + a_state];
                        const uint ba = outputs[pair.b_idx * fsm_params.states + b_state];
                        const auto p = payoff_for_actions(aa, ba, eval_params);
                        a_total += p.x;
                        b_total += p.y;
                        a_state = transitions[pair.a_idx * fsm_params.states * fsm_params.alphabet
                            + a_state * fsm_params.alphabet + ba];
                        b_state = transitions[pair.b_idx * fsm_params.states * fsm_params.alphabet
                            + b_state * fsm_params.alphabet + aa];
                    }
                    scores[gid].a_total = a_total;
                    scores[gid].b_total = b_total;
                    return;
                }
            }
            cycle_round[combined] = round;
            cycle_a[combined] = a_total;
            cycle_b[combined] = b_total;
        }

        const uint a_action = outputs[pair.a_idx * fsm_params.states + a_state];
        const uint b_action = outputs[pair.b_idx * fsm_params.states + b_state];
        const auto payoff = payoff_for_actions(a_action, b_action, eval_params);
        a_total += payoff.x;
        b_total += payoff.y;
        a_state = transitions[pair.a_idx * fsm_params.states * fsm_params.alphabet
            + a_state * fsm_params.alphabet
            + b_action];
        b_state = transitions[pair.b_idx * fsm_params.states * fsm_params.alphabet
            + b_state * fsm_params.alphabet
            + a_action];
    }
    scores[gid].a_total = a_total;
    scores[gid].b_total = b_total;
}

kernel void ca_batch(
    device const MatchPair* pairs [[buffer(0)]],
    device const uint* rule_tables [[buffer(1)]],
    device ScorePair* scores [[buffer(2)]],
    constant EvalParams& eval_params [[buffer(3)]],
    constant CaParams& ca_params [[buffer(4)]],
    uint gid [[thread_position_in_grid]]
) {
    if (gid >= eval_params.pair_count) {
        return;
    }
    const auto pair = pairs[gid];
    thread uint history_bits[CA_MAX_WINDOW];
    uint history_len = 0u;
    long a_total = 0;
    long b_total = 0;
    for (uint round = 0u; round < eval_params.rounds; round++) {
        const uint a_action = ca_action_for_strategy(rule_tables, ca_params, pair.a_idx, history_bits, history_len);
        const uint b_action = ca_action_for_strategy(rule_tables, ca_params, pair.b_idx, history_bits, history_len);
        const auto payoff = payoff_for_actions(a_action, b_action, eval_params);
        a_total += payoff.x;
        b_total += payoff.y;
        push_bit(history_bits, history_len, a_action);
        push_bit(history_bits, history_len, b_action);
    }
    scores[gid].a_total = a_total;
    scores[gid].b_total = b_total;
}

kernel void tm_batch(
    device const MatchPair* pairs [[buffer(0)]],
    device const uint* start_states [[buffer(1)]],
    device const TmTransition* transitions [[buffer(2)]],
    device ScorePair* scores [[buffer(3)]],
    constant EvalParams& eval_params [[buffer(4)]],
    constant TmParams& tm_params [[buffer(5)]],
    device TmHaltingPair* halting [[buffer(6)]],
    uint gid [[thread_position_in_grid]]
) {
    if (gid >= eval_params.pair_count) {
        return;
    }
    const auto pair = pairs[gid];
    thread uchar suffix_digits[TM_MAX_WIDTH];
    uint suffix_len = 1u;
    uint prefix_nonzero = 0u;
    suffix_digits[0] = 0u;
    long a_total = 0;
    long b_total = 0;
    bool a_all_halted = true;
    bool b_all_halted = true;
    for (uint round = 0u; round < eval_params.rounds; round++) {
        bool a_halted = true;
        bool b_halted = true;
        uint a_action = 0u;
        uint b_action = 0u;
        if (round != 0u) {
            thread uchar input_digits[TM_MAX_WIDTH];
            uint input_len = 0u;
            tm_input_digits(suffix_digits, suffix_len, prefix_nonzero, input_digits, input_len);
            a_action = tm_action_for_strategy(
                transitions,
                start_states,
                tm_params,
                pair.a_idx,
                input_digits,
                input_len,
                a_halted
            );
            b_action = tm_action_for_strategy(
                transitions,
                start_states,
                tm_params,
                pair.b_idx,
                input_digits,
                input_len,
                b_halted
            );
        }
        a_all_halted = a_all_halted && a_halted;
        b_all_halted = b_all_halted && b_halted;
        const auto payoff = payoff_with_timeouts(a_action, b_action, a_halted, b_halted, eval_params);
        a_total += payoff.x;
        b_total += payoff.y;
        const uint pair_digit = ((a_halted ? a_action : 0u) << 1u) | (b_halted ? b_action : 0u);
        tm_push_round(
            suffix_digits,
            suffix_len,
            prefix_nonzero,
            min(TM_MAX_WIDTH, tm_params.max_steps + 1u),
            pair_digit,
            tm_params.symbols
        );
    }
    scores[gid].a_total = a_total;
    scores[gid].b_total = b_total;
    halting[gid].a_all_halted = a_all_halted ? 1u : 0u;
    halting[gid].b_all_halted = b_all_halted ? 1u : 0u;
}
