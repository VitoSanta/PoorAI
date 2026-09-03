//! A plan as a graph of subgoals, each able to carry its own check.
//!
//! A plan was a list of sentences. `record_progress` recorded a claim, the
//! claim was reconciled at the end, and nothing between the two ever asked
//! whether a step had actually been finished -- so a long task was a long list
//! of assertions, and the only verification was the one at the very end, when
//! everything had already been spent.
//!
//! The boundary this project draws stays where it was. The harness still never
//! *infers* that a step is done: inferring would be the harness deciding the
//! task had progressed. What it can do is check a claim against a command,
//! which is the same thing it already does for completion -- and a command the
//! run is not allowed to execute is refused here exactly as it would be
//! anywhere else.

use serde::{Deserialize, Serialize};

/// One step of a plan, and what it depends on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Subgoal {
    pub statement: String,
    /// One-based steps that must be done first.
    ///
    /// A list is a graph where every step depends on the one before it. Most
    /// plans are not that shape: three files can be edited in any order and
    /// the fourth step needs all three, and a list cannot say so.
    #[serde(default)]
    pub depends_on: Vec<usize>,
    /// The command that says whether this step is done.
    ///
    /// Optional, and usually absent: most steps have no local check, and
    /// inventing one would be worse than having none. Where it exists it runs
    /// under the run's own policy, so a step cannot authorise a command the
    /// run could not otherwise execute.
    #[serde(default)]
    pub verify: Option<Vec<String>>,
}

/// What a plan's steps have come to.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepState {
    /// The deployment said it finished this step.
    pub claimed: bool,
    /// Its verifier ran and passed. `None` where it has no verifier: absent is
    /// not the same as failed, and reporting it as failed would make a plan
    /// without checks look like a plan that failed them.
    pub verified: Option<bool>,
}

/// A plan and the state of its steps.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<Subgoal>,
    pub state: Vec<StepState>,
}

impl Plan {
    pub fn new(steps: Vec<Subgoal>) -> Self {
        let state = vec![StepState::default(); steps.len()];
        Self { steps, state }
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether a step is finished: claimed, and passing its check if it has
    /// one.
    ///
    /// A step with a verifier that has not passed is not done however loudly
    /// it was claimed -- that is the whole reason for having one.
    pub fn done(&self, step: usize) -> bool {
        let Some(state) = self.state.get(step.wrapping_sub(1)) else {
            return false;
        };
        state.claimed && state.verified != Some(false)
    }

    /// Steps whose dependencies are all done.
    ///
    /// A dependency that does not exist is ignored rather than treated as
    /// unmet: a plan that refers to a step it does not have is a mistake in
    /// the plan, and blocking on it would strand the run over a typo.
    pub fn ready(&self) -> Vec<usize> {
        (1..=self.steps.len())
            .filter(|step| !self.done(*step))
            .filter(|step| {
                self.steps[step - 1]
                    .depends_on
                    .iter()
                    .filter(|dependency| **dependency >= 1 && **dependency <= self.steps.len())
                    .all(|dependency| self.done(*dependency))
            })
            .collect()
    }

    /// Steps not yet done that are waiting on something.
    pub fn blocked(&self) -> Vec<usize> {
        let ready = self.ready();
        (1..=self.steps.len())
            .filter(|step| !self.done(*step) && !ready.contains(step))
            .collect()
    }

    pub fn outstanding(&self) -> Vec<String> {
        (1..=self.steps.len())
            .filter(|step| !self.done(*step))
            .map(|step| format!("{step}. {}", self.steps[step - 1].statement))
            .collect()
    }

    pub fn done_count(&self) -> usize {
        (1..=self.steps.len())
            .filter(|step| self.done(*step))
            .count()
    }

    /// Records a claim. Returns false for a step the plan does not have.
    pub fn claim(&mut self, step: usize) -> bool {
        match self.state.get_mut(step.wrapping_sub(1)) {
            Some(state) => {
                state.claimed = true;
                true
            }
            None => false,
        }
    }

    pub fn record_verification(&mut self, step: usize, passed: bool) {
        if let Some(state) = self.state.get_mut(step.wrapping_sub(1)) {
            state.verified = Some(passed);
        }
    }

    /// The verifier a claimed step should be checked against, if it has one.
    pub fn verifier(&self, step: usize) -> Option<(String, Vec<String>)> {
        let command = self.steps.get(step.wrapping_sub(1))?.verify.as_ref()?;
        let (executable, args) = command.split_first()?;
        Some((executable.clone(), args.to_vec()))
    }
}

/// Reads a plan from what a deployment answered.
///
/// Two shapes are accepted, because a plan is worth having in either. A list
/// of strings is the older form and still the common one; a list of objects
/// carries dependencies and a check. A deployment that answers with the first
/// is not failing -- it is planning without a graph, which is what every plan
/// in this project has been until now.
pub fn parse_steps(value: &serde_json::Value, limit: usize) -> Vec<Subgoal> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            serde_json::Value::String(statement) => Some(Subgoal {
                statement: statement.trim().to_string(),
                ..Default::default()
            }),
            serde_json::Value::Object(_) => {
                let statement = item
                    .get("step")
                    .or_else(|| item.get("statement"))
                    .and_then(|value| value.as_str())?
                    .trim()
                    .to_string();
                Some(Subgoal {
                    statement,
                    depends_on: item
                        .get("depends_on")
                        .and_then(|value| value.as_array())
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(serde_json::Value::as_u64)
                                .map(|value| value as usize)
                                .collect()
                        })
                        .unwrap_or_default(),
                    verify: item
                        .get("verify")
                        .and_then(|value| value.as_array())
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(|value| value.as_str().map(str::to_string))
                                .collect::<Vec<_>>()
                        })
                        .filter(|command| !command.is_empty()),
                })
            }
            _ => None,
        })
        .filter(|step| !step.statement.is_empty())
        .take(limit)
        .collect()
}
