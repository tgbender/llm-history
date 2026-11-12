//! Showing messages in their conversation, and reading one session's turns.
use super::*;
use crate::output::{ContextRow, TurnRow};
use llm_history_types::query::Message;

/// The session reader prints a turn of up to this many characters whole.
const TURN_CHARS: usize = 1000;
/// The same bound for the session's last answer.
const ANSWER_CHARS: usize = 2000;
/// Opening turns kept when a session is too long to print whole.
const OPENING_TURNS: usize = 3;

pub(crate) fn show(ctx: &Ctx, args: ShowArgs) -> Result<()> {
    match args.session {
        Some(session) => read_session(ctx, session, &args),
        None => show_messages(ctx, &args),
    }
}

/// Divides `total` among `needs`. Nothing gets more than it needs, and the rest share
/// what the short ones leave.
fn shares(needs: &[usize], total: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..needs.len()).collect();
    order.sort_by_key(|&at| needs[at]);
    let mut shares = vec![0; needs.len()];
    let mut left = total;
    for (done, &at) in order.iter().enumerate() {
        shares[at] = needs[at].min(left / (needs.len() - done));
        left -= shares[at];
    }
    shares
}

/// Character budgets for one block's messages. The requested messages get up to two
/// thirds, their neighbors share the rest, and whatever the neighbors leave goes back.
fn message_budgets(needs: &[usize], requested: &[bool], budget: usize) -> Vec<usize> {
    let pick = |want: bool| -> Vec<usize> {
        (0..needs.len())
            .filter(|&at| requested[at] == want)
            .collect()
    };
    let (targets, neighbors) = (pick(true), pick(false));
    let split = |members: &[usize], total: usize| {
        shares(
            &members.iter().map(|&at| needs[at]).collect::<Vec<_>>(),
            total,
        )
    };
    let reserved = if neighbors.is_empty() {
        budget
    } else {
        budget.saturating_mul(2) / 3
    };
    let first: usize = split(&targets, reserved).iter().sum();
    let around = split(&neighbors, budget - first);
    let spent: usize = around.iter().sum();
    let mut budgets = vec![0; needs.len()];
    for (at, share) in targets.iter().zip(split(&targets, budget - spent)) {
        budgets[*at] = share;
    }
    for (at, share) in neighbors.iter().zip(around) {
        budgets[*at] = share;
    }
    budgets
}

fn conversation_order(message: &Message) -> (i64, i64) {
    (message.session_seq, message.id.parse().unwrap_or(i64::MAX))
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

fn show_messages(ctx: &Ctx, args: &ShowArgs) -> Result<()> {
    let history = ctx.history();
    let mut requested = Vec::new();
    for id in &args.id {
        if !requested.contains(id) {
            requested.push(*id);
        }
    }
    // Windows that share a message become one block, so nothing prints twice.
    let mut blocks: Vec<Vec<Message>> = Vec::new();
    for id in &requested {
        let mut window = history
            .context(*id, args.before, args.after, &args.profile)
            .with_context(|| format!("message {id}"))?;
        let mut first = None;
        let mut at = 0;
        while at < blocks.len() {
            let shared = blocks[at]
                .iter()
                .any(|m| window.iter().any(|w| w.message_no == m.message_no));
            if shared {
                window.append(&mut blocks.remove(at));
                first.get_or_insert(at);
            } else {
                at += 1;
            }
        }
        window.sort_by_key(conversation_order);
        window.dedup_by_key(|m| m.message_no);
        blocks.insert(first.unwrap_or(blocks.len()), window);
    }
    let needs: Vec<usize> = blocks
        .iter()
        .map(|block| block.iter().map(|m| chars(&m.text)).sum())
        .collect();
    let block_budgets = shares(&needs, args.max_output_chars);
    let mut contexts = Vec::new();
    for (at, (block, block_budget)) in blocks.into_iter().zip(block_budgets).enumerate() {
        let targets: Vec<usize> = (0..block.len())
            .filter(|&at| requested.contains(&block[at].message_no))
            .collect();
        let is_target: Vec<bool> = (0..block.len()).map(|at| targets.contains(&at)).collect();
        let budgets = if args.full {
            vec![usize::MAX; block.len()]
        } else {
            let needs: Vec<usize> = block.iter().map(|m| chars(&m.text)).collect();
            message_budgets(&needs, &is_target, block_budget)
        };
        let scope = json!({"profile":block[0].profile,"session_id":block[0].session_no,"source":block[0].source});
        if !ctx.json && at > 0 {
            println!();
        }
        let mut rows = Vec::new();
        for (index, mut message) in block.into_iter().enumerate() {
            let budget = budgets[index];
            // A guardian prompt ends with the reviewed action, so cutting its end would lose it.
            let action = (message.session_kind == "guardian" && chars(&message.text) > budget)
                .then(|| llm_history_core::guardian_action(&message.text))
                .flatten()
                .map(|action| format!("[Quoted transcript omitted; --full shows it]\n{action}"));
            message.text = bounded(
                action.as_deref().unwrap_or(&message.text),
                budget,
                !args.no_redact,
            );
            let ts = message.timestamp_utc.unwrap_or(message.ts);
            if ctx.json {
                rows.push(ContextRow {
                    id: message.message_no,
                    role: message.role,
                    ts,
                    text: message.text,
                    target: is_target[index].then_some(true),
                });
                continue;
            }
            let marker = if is_target[index] {
                "*".into()
            } else {
                let nearest = targets
                    .iter()
                    .map(|&target| index as i64 - target as i64)
                    .min_by_key(|offset| offset.abs())
                    .unwrap_or(0);
                format!("{nearest:+}")
            };
            println!(
                "{marker}  {ts}  {}  {}  {}/{}  id={}\n  {}",
                message.profile,
                message.project,
                message.role,
                message.authorship,
                message.message_no,
                message.text,
            );
        }
        if ctx.json {
            let mut context = scope;
            context["messages"] = serde_json::to_value(rows)?;
            contexts.push(context);
        }
    }
    if ctx.json {
        // One requested message keeps the single-context shape.
        let result = if requested.len() == 1 {
            contexts.remove(0)
        } else {
            json!({ "contexts": contexts })
        };
        emit(&result, true)?;
    }
    Ok(())
}

/// A text over `max` characters keeps its start and end around a note of what was cut.
/// Returns the text and how many characters it lost.
fn clip(text: &str, max: usize) -> (String, usize) {
    let all: Vec<char> = text.chars().collect();
    if all.len() <= max {
        return (text.to_owned(), 0);
    }
    let start = max * 7 / 10;
    let end = max - start;
    let cut = all.len() - max;
    let piece = |range: &[char]| range.iter().collect::<String>();
    (
        format!(
            "{} [… {cut} characters cut …] {}",
            piece(&all[..start]).trim_end(),
            piece(&all[all.len() - end..]).trim_start()
        ),
        cut,
    )
}

/// Which turns fit: the first `.0` and those from index `.1` on. Everything prints when
/// the two meet. The first turn always prints, and the latest turns fill what is left.
fn select_turns(sizes: &[usize], budget: usize, recent: Option<usize>) -> (usize, usize) {
    let count = sizes.len();
    let whole = sizes.iter().sum::<usize>() <= budget;
    if whole && recent.is_none_or(|recent| count <= recent + 1) {
        return (count, count);
    }
    let opening = if recent.is_some() { 1 } else { OPENING_TURNS };
    let (mut used, mut head) = (0, 0);
    while head < opening.min(count) && (head == 0 || used + sizes[head] <= budget) {
        used += sizes[head];
        head += 1;
    }
    let mut tail = count;
    while tail > head && count - tail < recent.unwrap_or(count) && used + sizes[tail - 1] <= budget
    {
        used += sizes[tail - 1];
        tail -= 1;
    }
    (head, tail)
}

fn read_session(ctx: &Ctx, session: i64, args: &ShowArgs) -> Result<()> {
    let reading = ctx.history().session_turns(session, &args.profile)?;
    let total = reading.turns.len();
    let from = match args.from {
        Some(id) => reading
            .turns
            .iter()
            .position(|turn| turn.message_no == id)
            .with_context(|| format!("message {id} is not one of this session's user turns"))?,
        None => 0,
    };
    let clipped = |message: &Message, max: usize| {
        let text = if args.no_redact {
            message.text.clone()
        } else {
            llm_history_core::redact(&message.text)
        };
        clip(&text, max)
    };
    let answer = reading.last_answer.as_ref().map(|answer| {
        let (text, cut) = clipped(answer, ANSWER_CHARS.min(args.max_output_chars / 4));
        (answer, text, cut)
    });
    let turns: Vec<(&Message, String, usize)> = reading.turns[from..]
        .iter()
        .map(|turn| {
            let (text, cut) = clipped(turn, TURN_CHARS);
            (turn, text, cut)
        })
        .collect();
    let budget = args
        .max_output_chars
        .saturating_sub(answer.as_ref().map_or(0, |(_, text, _)| chars(text)));
    let sizes: Vec<usize> = turns.iter().map(|(_, text, _)| chars(text)).collect();
    let (head, tail) = select_turns(&sizes, budget, args.recent.map(usize::from));
    let skipped = (head < tail).then(|| (&turns[head].0, &turns[tail - 1].0, tail - head));
    let shown = turns.len() - skipped.map_or(0, |(_, _, count)| count);

    // Only a first turn larger than the whole budget is shortened further here.
    let mut left = budget;
    let mut rows: Vec<(&Message, Option<usize>, String, usize)> = Vec::new();
    let mut gap_at = None;
    for (index, (turn, text, cut)) in turns.iter().enumerate() {
        if (head..tail).contains(&index) {
            gap_at = Some(rows.len());
            continue;
        }
        let text = bounded(text, left, false);
        left -= chars(&text);
        rows.push((*turn, Some(from + index + 1), text, *cut));
    }
    // The answer takes its place in the conversation, which is last unless a turn followed it.
    if let Some((answer, text, cut)) = answer {
        let at = rows
            .iter()
            .position(|row| conversation_order(row.0) > conversation_order(answer))
            .unwrap_or(rows.len());
        rows.insert(at, (answer, None, text, cut));
        if let Some(gap) = gap_at.as_mut().filter(|gap| at <= **gap) {
            *gap += 1;
        }
    }
    let any_cut = rows.iter().any(|row| row.3 > 0);

    let about = rows.first().map(|row| row.0);
    if ctx.json {
        let mut result = json!({"session_id":session,"turns":total});
        if let Some(message) = about {
            result["profile"] = json!(message.profile);
            result["source"] = json!(message.source);
            result["project"] = json!(message.project);
        }
        if let Some((first, last, count)) = skipped {
            result["skipped"] =
                json!({"turns":count,"first_id":first.message_no,"last_id":last.message_no});
        }
        result["messages"] = serde_json::to_value(
            rows.into_iter()
                .map(|(message, turn, text, cut)| TurnRow {
                    id: message.message_no,
                    role: message.role.clone(),
                    ts: message.timestamp_utc.clone().unwrap_or(message.ts.clone()),
                    text,
                    turn,
                    cut: (cut > 0).then_some(cut),
                })
                .collect::<Vec<_>>(),
        )?;
        return emit(&result, true);
    }
    let plural = if total == 1 { "" } else { "s" };
    match about {
        Some(message) => println!(
            "session {session}  {}  {}  {total} user turn{plural}",
            message.profile, message.project
        ),
        None => println!("session {session}  {total} user turn{plural}"),
    }
    let gap = skipped.map(|(first, last, count)| {
        format!(
            "[turns {} to {} skipped, ids {} to {}]",
            from + head + 1,
            from + head + count,
            first.message_no,
            last.message_no
        )
    });
    let printed = rows.len();
    for (at, (message, turn, text, _)) in rows.into_iter().enumerate() {
        if gap_at == Some(at)
            && let Some(gap) = &gap
        {
            println!("{gap}");
        }
        let ts = message.timestamp_utc.as_deref().unwrap_or(&message.ts);
        let label = turn.map_or("answer".into(), |turn| turn.to_string());
        println!("{label}  {ts}  id={}\n  {text}", message.message_no);
    }
    // Nothing after the gap fit, so it closes the listing.
    if gap_at == Some(printed)
        && let Some(gap) = &gap
    {
        println!("{gap}");
    }
    let mut notes = Vec::new();
    if gap.is_some() {
        notes.push(format!(
            "{shown} of {total} turns shown; --from ID starts at a skipped turn, and \
             --max-output-chars raises the budget."
        ));
    }
    if any_cut {
        notes.push("show ID --full prints a cut message whole.".into());
    }
    if !notes.is_empty() {
        println!("{}", notes.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_messages_leave_their_share_to_long_ones() {
        assert_eq!(shares(&[10, 500, 500], 310), [10, 150, 150]);
        assert_eq!(shares(&[10, 20], 100), [10, 20]);
        assert!(shares(&[], 100).is_empty());
        // The requested message keeps two thirds until its neighbors need less.
        let requested = [false, true, false];
        assert_eq!(
            message_budgets(&[900, 900, 900], &requested, 900),
            [150, 600, 150]
        );
        assert_eq!(
            message_budgets(&[10, 900, 20], &requested, 900),
            [10, 870, 20]
        );
        assert_eq!(message_budgets(&[900], &[true], 600), [600]);
    }

    #[test]
    fn a_long_turn_keeps_its_start_and_end() {
        let (text, cut) = clip(&format!("ask {} end", "x".repeat(2000)), 1000);
        assert!(
            text.starts_with("ask x") && text.ends_with("x end"),
            "{text}"
        );
        assert_eq!(cut, 1008);
        assert!(text.contains("[… 1008 characters cut …]"), "{text}");
        assert_eq!(clip("short", 1000), ("short".to_owned(), 0));
    }

    #[test]
    fn a_long_session_keeps_its_opening_and_latest_turns() {
        // Everything fits.
        assert_eq!(select_turns(&[100; 5], 1000, None), (5, 5));
        // Three opening turns, then as many of the latest as fit.
        assert_eq!(select_turns(&[100; 20], 1000, None), (3, 13));
        // The opening turn and the two latest, even when all would fit.
        assert_eq!(select_turns(&[100; 20], 5000, Some(2)), (1, 18));
        assert_eq!(select_turns(&[100; 3], 5000, Some(2)), (3, 3));
        // A budget smaller than one turn still prints the first.
        assert_eq!(select_turns(&[100; 4], 50, None), (1, 4));
        assert_eq!(select_turns(&[], 50, None), (0, 0));
    }
}
