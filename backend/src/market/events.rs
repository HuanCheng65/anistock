//! Market events (§5): one structure for everything that moves beliefs or activity. Internal events now
//! (premiere, finale, delisting warning, 热度快讯 from episode comments); news later through the same entry.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Credibility {
    Official,
    Media,
    Rumor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Nature {
    /// shifts beliefs until the next close (the hidden value then absorbs or contradicts it)
    Fundamental,
    /// decays with its half-life
    Sentiment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub company_id: i64,
    /// premiere | finale | delist_warning | flash | news...
    pub kind: String,
    /// -1, 0, 1
    pub direction: i8,
    /// 0-10
    pub magnitude: f64,
    pub credibility: Credibility,
    pub source: String,
    /// unix seconds when it is announced (effective from then, for players and NPCs alike)
    pub at: i64,
    pub nature: Nature,
    /// hours (sentiment events)
    pub half_life_h: f64,
    /// the news headline shown in the game
    pub title: String,
    /// revisions: the log change of the public value (sign included)
    #[serde(default)]
    pub change: f64,
}

impl Event {
    pub fn credibility_weight(&self, w: &[f64; 3]) -> f64 {
        match self.credibility {
            Credibility::Official => w[0],
            Credibility::Media => w[1],
            Credibility::Rumor => w[2],
        }
    }

    /// Fraction of the event's belief shock still in force at `t` (`next_close`: unix seconds of the first
    /// close after the event).
    pub fn belief_weight(&self, t: i64, next_close: i64) -> f64 {
        if t < self.at {
            return 0.0;
        }
        match self.nature {
            Nature::Fundamental => {
                if t < next_close {
                    1.0
                } else {
                    0.0
                }
            }
            Nature::Sentiment => 0.5f64.powf((t - self.at) as f64 / 3600.0 / self.half_life_h.max(0.01)),
        }
    }

    /// Activity weight at `t`, decaying with `half_life_h`.
    pub fn activity_weight(&self, t: i64, half_life_h: f64) -> f64 {
        if t < self.at {
            return 0.0;
        }
        0.5f64.powf((t - self.at) as f64 / 3600.0 / half_life_h.max(0.01))
    }
}
