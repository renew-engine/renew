//! What a divergence looks like when it is written down.
//!
//! A desync is the one failure this protocol cannot prevent and must
//! therefore explain. The report exists so the first question a developer
//! asks — *whose fault is it* — is answered by the data rather than by a
//! debugger, and the answer is one bit wide: [`DesyncReport::inputs_agree`].

use crate::{MAX_PEERS, PeerId, PeerSet, SessionStats};

const PEERS: usize = MAX_PEERS as usize;

/// Everything known about a divergence at the tick it was caught.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesyncReport {
    /// The digested tick at which the fingerprints disagreed.
    pub tick: u64,
    pub local: PeerId,
    pub peer_count: u8,
    pub local_state_digest: u64,
    /// One entry per seat; `None` where that peer has not reported.
    pub peer_state_digests: [Option<u64>; PEERS],
    pub local_input_digest: u64,
    pub peer_input_digests: [Option<u64>; PEERS],
    /// The most recent tick at which every reporting peer agreed.
    ///
    /// The divergence is inside `(last_agreed_tick, tick]` — at most
    /// `digest_period` ticks wide, which turns that parameter from a
    /// bandwidth knob into a bisect knob and prices it: at sixty hertz
    /// and a period of thirty, the window to search is half a second of
    /// simulation.
    pub last_agreed_tick: Option<u64>,
    pub agreement_digest: u64,
    pub content: u64,
    pub rules: u64,
    pub seed: u64,
    /// Arrival- and rate-dependent counters, carried as evidence and
    /// **never folded into anything**.
    pub stats: SessionStats,
}

impl DesyncReport {
    /// The peers whose state fingerprint differs from this machine's.
    ///
    /// With three or more peers this names the odd one out, which is the
    /// blame that majority corroboration would have bought — recorded
    /// here rather than acted on, because a majority continuing while a
    /// minority plays a different game is a silent fork by vote.
    #[must_use]
    pub fn dissenters(&self) -> PeerSet {
        let mut set = PeerSet::EMPTY;
        for peer in PeerSet::of_count(self.peer_count).iter() {
            if peer == self.local {
                continue;
            }
            if let Some(Some(theirs)) = self.peer_state_digests.get(usize::from(peer.index()))
                && *theirs != self.local_state_digest
            {
                set = set.with(peer);
            }
        }
        set
    }

    /// **The one-bit classification, and the reason an input digest is on
    /// the wire at all.**
    ///
    /// `true` — every reporting peer ran identical inputs and produced
    /// different state. The network is exonerated *by evidence* rather
    /// than by assertion, and the bug is in the simulation, the
    /// toolchain, the build profile, or the content. The next step is the
    /// cross-platform determinism lane, or `content` and `rules`.
    ///
    /// `false` — the peers ran different inputs, and the state digest is
    /// a symptom rather than the disease. The bug is here, in the
    /// caller's submit discipline, or in transport — and [`Self::stats`]
    /// is in this report for exactly this branch.
    #[must_use]
    pub fn inputs_agree(&self) -> bool {
        PeerSet::of_count(self.peer_count)
            .iter()
            .filter(|peer| *peer != self.local)
            .filter_map(|peer| {
                self.peer_input_digests
                    .get(usize::from(peer.index()))
                    .copied()
            })
            .flatten()
            .all(|theirs| theirs == self.local_input_digest)
    }

    /// Whether any remote reported at all for this tick.
    ///
    /// A report about a tick nobody witnessed compares nothing, and must
    /// never read as agreement — the same discipline the determinism
    /// lane's inconclusive verdict keeps.
    #[must_use]
    pub fn witnessed(&self) -> bool {
        self.peer_state_digests.iter().any(Option::is_some)
    }

    /// The machine-readable form, so a caller can parse a desync report
    /// rather than format one itself.
    #[must_use]
    pub const fn json(&self) -> DesyncReportJson<'_> {
        DesyncReportJson(self)
    }
}

/// The JSON face of a [`DesyncReport`].
///
/// `schema_version` is the first field and it is `1`. Written by hand
/// rather than derived, for the reason the rest of this tree writes its
/// JSON by hand: a derive makes the wire shape a consequence of field
/// order, and this one is read by tools.
#[derive(Clone, Copy, Debug)]
pub struct DesyncReportJson<'a>(&'a DesyncReport);

impl core::fmt::Display for DesyncReportJson<'_> {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let report = self.0;
        write!(
            out,
            r#"{{"schema_version":1,"tick":{},"local":{},"peer_count":{},"inputs_agree":{},"witnessed":{},"#,
            report.tick,
            report.local.index(),
            report.peer_count,
            report.inputs_agree(),
            report.witnessed()
        )?;
        write!(
            out,
            r#""local_state_digest":"{:#018x}","local_input_digest":"{:#018x}","#,
            report.local_state_digest, report.local_input_digest
        )?;
        match report.last_agreed_tick {
            Some(tick) => write!(out, r#""last_agreed_tick":{tick},"#)?,
            None => write!(out, r#""last_agreed_tick":null,"#)?,
        }
        write!(out, r#""dissenters":["#)?;
        for (position, peer) in report.dissenters().iter().enumerate() {
            if position > 0 {
                write!(out, ",")?;
            }
            write!(out, "{}", peer.index())?;
        }
        write!(out, r#"],"peers":["#)?;
        for (position, peer) in PeerSet::of_count(report.peer_count).iter().enumerate() {
            if position > 0 {
                write!(out, ",")?;
            }
            let seat = usize::from(peer.index());
            let state = report.peer_state_digests.get(seat).copied().flatten();
            let input = report.peer_input_digests.get(seat).copied().flatten();
            write!(out, r#"{{"seat":{},"#, peer.index())?;
            match state {
                Some(value) => write!(out, r#""state_digest":"{value:#018x}","#)?,
                None => write!(out, r#""state_digest":null,"#)?,
            }
            match input {
                Some(value) => write!(out, r#""input_digest":"{value:#018x}"}}"#)?,
                None => write!(out, r#""input_digest":null}}"#)?,
            }
        }
        write!(
            out,
            r#"],"agreement_digest":"{:#018x}","content":"{:#018x}","rules":"{:#018x}","seed":{},"#,
            report.agreement_digest, report.content, report.rules, report.seed
        )?;
        write!(
            out,
            r#""stats":{{"datagrams_refused":{},"frames_repeated":{},"frames_contradicted":{},"frames_out_of_window":{},"digests_contradicted":{},"stall_pumps":{}}}}}"#,
            report.stats.datagrams_refused,
            report.stats.frames_repeated,
            report.stats.frames_contradicted,
            report.stats.frames_out_of_window,
            report.stats.digests_contradicted,
            report.stats.stall_pumps
        )
    }
}
