//! Where a job should run when the node that received it has peers. Pure: it only compares numbers.
//!
//! The expected wait of a node is the jobs in front of the new one plus the new one, each taking about as long as
//! this job would at the node's measured speed. A node that cannot take the job at all (paused, or no such model)
//! has no wait. The node that received the job keeps it unless it is clearly worse than a peer.

/// A peer must beat this node's wait by this factor before the job is forwarded.
pub const RATIO: f32 = 1.5;
/// What sending the job to a peer and getting the answer back costs on top, in seconds.
pub const FORWARD_COST_S: f32 = 0.5;
/// Used for a node that has not measured its speed yet.
const DEFAULT_CHAT_TOK_S: f32 = 8.0;
const DEFAULT_EMBED_PER_S: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Job {
    /// The most tokens the answer may have.
    Chat { max_tokens: u32 },
    /// How many texts.
    Embed { texts: u32 },
}

/// What one node can say about itself for a job: from its own counters, or from a peer's status.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    /// Jobs it has accepted and not finished.
    pub load: u32,
    pub paused: bool,
    /// Tokens a second for chat, texts a second for search. Zero when not measured.
    pub speed: f32,
    /// Whether it has the model for this job.
    pub serves: bool,
}

/// Seconds until the job would be done here, or None when this node cannot take it now.
pub fn wait_s(job: Job, n: &Node) -> Option<f32> {
    if n.paused || !n.serves {
        return None;
    }
    let (work, speed) = match job {
        // Answers are rarely as long as the limit, so a long limit counts for less.
        Job::Chat { max_tokens } => (max_tokens.clamp(16, 200) as f32, if n.speed > 0.0 { n.speed } else { DEFAULT_CHAT_TOK_S }),
        Job::Embed { texts } => (texts.max(1) as f32, if n.speed > 0.0 { n.speed } else { DEFAULT_EMBED_PER_S }),
    };
    Some((n.load as f32 + 1.0) * work / speed)
}

/// The peers to try, best first, or nothing when the job should run here. `peers` are the ones that can be reached
/// and have a fresh status, as (id, node).
pub fn choose(job: Job, local: &Node, peers: &[(String, Node)]) -> Vec<String> {
    let mut ranked: Vec<(f32, &String)> = peers.iter().filter_map(|(id, n)| wait_s(job, n).map(|w| (w + FORWARD_COST_S, id))).collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    match wait_s(job, local) {
        // This node cannot take it: every peer that can is a choice.
        None => ranked.into_iter().map(|(_, id)| id.clone()).collect(),
        Some(mine) => ranked.into_iter().filter(|(w, _)| mine > RATIO * w + FORWARD_COST_S).map(|(_, id)| id.clone()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAT: Job = Job::Chat { max_tokens: 200 };

    fn node(load: u32, speed: f32) -> Node {
        Node { load, paused: false, speed, serves: true }
    }

    fn peers(list: &[(&str, Node)]) -> Vec<(String, Node)> {
        list.iter().map(|(id, n)| (id.to_string(), *n)).collect()
    }

    #[test]
    fn an_idle_node_keeps_its_job() {
        assert!(choose(CHAT, &node(0, 30.0), &peers(&[("a", node(0, 30.0))])).is_empty());
        // Even when the peer is a little faster.
        assert!(choose(CHAT, &node(0, 30.0), &peers(&[("a", node(0, 40.0))])).is_empty());
    }

    #[test]
    fn a_busy_node_forwards_to_an_idle_peer() {
        // 3 jobs in front of 6.7 s each is 27 s here, against 6.7 s and the hop there.
        assert_eq!(choose(CHAT, &node(3, 30.0), &peers(&[("a", node(0, 30.0))])), vec!["a"]);
    }

    #[test]
    fn a_peer_has_to_be_clearly_better() {
        // One job in front here is twice the wait of an idle peer of the same speed: 13.3 s against 6.7 s plus the hop.
        assert_eq!(choose(CHAT, &node(1, 30.0), &peers(&[("a", node(0, 30.0))])), vec!["a"]);
        // The same peer with 1 job of its own is no better.
        assert!(choose(CHAT, &node(1, 30.0), &peers(&[("a", node(1, 30.0))])).is_empty());
        // A peer that is 1.4 times faster is not enough, the wait here has to be over 1.5 times.
        assert!(choose(CHAT, &node(0, 30.0), &peers(&[("a", node(0, 42.0))])).is_empty());
        // 2 times faster is, once the hop is paid.
        assert_eq!(choose(CHAT, &node(0, 10.0), &peers(&[("a", node(0, 20.0))])), vec!["a"]);
    }

    #[test]
    fn the_hop_counts_for_short_jobs() {
        // Searching 1 text takes 0.05 s here, a peer cannot beat that however idle it is.
        let e = Job::Embed { texts: 1 };
        assert!(choose(e, &node(1, 20.0), &peers(&[("a", node(0, 200.0))])).is_empty());
        // 400 texts behind 2 jobs is worth the hop.
        let e = Job::Embed { texts: 400 };
        assert_eq!(choose(e, &node(2, 20.0), &peers(&[("a", node(0, 20.0))])), vec!["a"]);
    }

    #[test]
    fn a_paused_node_sends_everything_on() {
        let mut me = node(0, 30.0);
        me.paused = true;
        assert_eq!(choose(CHAT, &me, &peers(&[("a", node(5, 30.0))])), vec!["a"]);
    }

    #[test]
    fn a_node_without_the_model_sends_the_job_on_even_to_a_slow_peer() {
        let mut me = node(0, 0.0);
        me.serves = false;
        assert_eq!(choose(CHAT, &me, &peers(&[("slow", node(4, 2.0))])), vec!["slow"]);
        // And embeddings are judged apart from chat: this node may not serve one and serve the other.
        assert!(choose(Job::Embed { texts: 10 }, &node(0, 20.0), &peers(&[("slow", node(4, 2.0))])).is_empty());
    }

    #[test]
    fn peers_that_cannot_take_the_job_are_never_chosen() {
        let mut paused = node(0, 100.0);
        paused.paused = true;
        let mut none = node(0, 100.0);
        none.serves = false;
        let mut me = node(0, 30.0);
        me.paused = true;
        assert!(choose(CHAT, &me, &peers(&[("paused", paused), ("none", none)])).is_empty());
        assert_eq!(choose(CHAT, &me, &peers(&[("paused", paused), ("none", none), ("ok", node(1, 10.0))])), vec!["ok"]);
    }

    #[test]
    fn peers_are_tried_best_first() {
        let list = peers(&[("slow", node(0, 20.0)), ("busy", node(2, 75.0)), ("fast", node(0, 60.0))]);
        assert_eq!(choose(CHAT, &node(6, 30.0), &list), vec!["fast", "busy", "slow"]);
        // A peer that does not beat this node by enough is left out of the fallbacks.
        assert_eq!(choose(CHAT, &node(1, 35.0), &list), vec!["fast"]);
    }

    #[test]
    fn nothing_is_chosen_without_peers() {
        assert!(choose(CHAT, &node(9, 30.0), &[]).is_empty());
        let mut me = node(0, 30.0);
        me.serves = false;
        assert!(choose(CHAT, &me, &[]).is_empty());
    }

    #[test]
    fn an_unmeasured_speed_uses_a_default() {
        // Both unmeasured and equal: stays.
        assert!(choose(CHAT, &node(0, 0.0), &peers(&[("a", node(0, 0.0))])).is_empty());
        assert_eq!(wait_s(CHAT, &node(0, 0.0)), Some(25.0));
    }
}
