use std::ops::Not;

use log::warn;

use crate::basic_types::ConstraintOperationError;
use crate::basic_types::Inconsistency;
use crate::basic_types::PropositionalConjunction;
use crate::conjunction;
use crate::containers::KeyedVec;
use crate::containers::StorageKey;
use crate::engine::conflict_analysis::Mode;
use crate::engine::nogoods::Lbd;
use crate::engine::opaque_domain_event::OpaqueDomainEvent;
use crate::engine::predicates::predicate::Predicate;
use crate::engine::propagation::propagation_context::HasAssignments;
use crate::engine::propagation::EnqueueDecision;
use crate::engine::propagation::LocalId;
use crate::engine::propagation::PropagationContext;
use crate::engine::propagation::PropagationContextMut;
use crate::engine::propagation::Propagator;
use crate::engine::propagation::PropagatorInitialisationContext;
use crate::engine::propagation::ReadDomains;
use crate::engine::reason::Reason;
use crate::engine::variables::DomainId;
use crate::engine::Assignments;
use crate::engine::EventSink;
use crate::engine::IntDomainEvent;
use crate::predicate;
use crate::pumpkin_assert_advanced;
use crate::pumpkin_assert_moderate;
use crate::pumpkin_assert_simple;

// TODO:
// Define data structures.
// Notify should accumulate things to propagate.
// Propagating goes through the data structures?
//
// todo: for now we do not accumulate the updates, we simply update as we enqueued them.
// todo: could specialise the propagator for binary integers. This is a general comment.

// Need also LBD, and nogood clean ups! Remove root level true predicates.

// Todo: add predicate compression for 1) logging nogoods, and 2) smaller memory footprint.

// Current data structure:
// [domain_id][operation] -> [list of A], where A is list of watchers, where a watcher where is:

// Say for operation >=
// A is a list of pairs (bound, nogood)

// Todo: the way the hashmap is used is not efficient.

/// A struct which represents a nogood (i.e. a list of [`Predicate`]s which cannot all be true at
/// the same time).
///
/// It additionally contains certain fields related to how the clause was created/activity.
#[derive(Default, Clone, Debug)]
struct Nogood {
    predicates: PropositionalConjunction,
    is_learned: bool,
    lbd: u32,
    is_protected: bool,
    is_deleted: bool,
    block_bumps: bool,
    activity: f32,
}

impl Nogood {
    fn new_learned_nogood(predicates: PropositionalConjunction, lbd: u32) -> Self {
        Nogood {
            predicates,
            is_learned: true,
            lbd,
            ..Default::default()
        }
    }

    fn new_permanent_nogood(predicates: PropositionalConjunction) -> Self {
        Nogood {
            predicates,
            ..Default::default()
        }
    }
}

/// A propagator which propagates nogoods (i.e. a list of [`Predicate`]s which cannot all be true
/// at the same time).
#[derive(Clone, Debug)]
pub(crate) struct NogoodPropagator {
    /// The list of currently stored nogoods
    nogoods: KeyedVec<NogoodId, Nogood>,
    /// Nogoods which are permanently present
    permanent_nogoods: Vec<NogoodId>,
    /// The ids of the nogoods sorted based on whether they have a "low" LBD score or a "high" LBD
    /// score.
    learned_nogood_ids: LearnedNogoodIds,
    /// Ids which have been deleted and can now be re-used
    delete_ids: Vec<NogoodId>,
    /// The trail index is used to determine the domains of the variables since last time.
    last_index_on_trail: usize,
    /// Indicates whether the nogood propagator is in an infeasible state
    is_in_infeasible_state: bool,
    /// Watch lists for the nogood propagator.
    // TODO: could improve the data structure for watching.
    watch_lists: KeyedVec<DomainId, NogoodWatchList>,
    enqueued_updates: EventSink,
    lbd_helper: Lbd,
    activity_bump_increment: f32,
    parameters: LearningOptions,
    bumped_nogoods: Vec<NogoodId>,
}

#[derive(Default, Debug, Clone)]
struct LearnedNogoodIds {
    low_lbd: Vec<NogoodId>,
    high_lbd: Vec<NogoodId>,
}

#[derive(Debug, Copy, Clone)]
struct LearningOptions {
    max_activity: f32,
    activity_decay_factor: f32,
    limit_num_high_lbd_nogoods: usize,
    lbd_threshold: u32,
    nogood_sorting_strategy: LearnedNogoodSortingStrategy,
}

impl Default for LearningOptions {
    fn default() -> Self {
        Self {
            max_activity: 1e20,
            activity_decay_factor: 0.99,
            limit_num_high_lbd_nogoods: 4000,
            nogood_sorting_strategy: LearnedNogoodSortingStrategy::Lbd,
            lbd_threshold: 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LearnedNogoodSortingStrategy {
    #[allow(dead_code)]
    Activity,
    Lbd,
}

impl std::fmt::Display for LearnedNogoodSortingStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            LearnedNogoodSortingStrategy::Lbd => write!(f, "lbd"),
            LearnedNogoodSortingStrategy::Activity => write!(f, "activity"),
        }
    }
}

impl Default for NogoodPropagator {
    fn default() -> Self {
        Self {
            nogoods: Default::default(),
            permanent_nogoods: Default::default(),
            learned_nogood_ids: Default::default(),
            delete_ids: Default::default(),
            last_index_on_trail: Default::default(),
            is_in_infeasible_state: Default::default(),
            watch_lists: Default::default(),
            enqueued_updates: Default::default(),
            lbd_helper: Lbd::default(),
            parameters: LearningOptions::default(),
            activity_bump_increment: 1.0,
            bumped_nogoods: Default::default(),
        }
    }
}

impl NogoodPropagator {
    /// Does simple preprocessing, modifying the input nogood by:
    ///     1. Removing duplicate predicates.
    ///     2. Removing satisfied predicates at the root.
    ///     3. Detecting predicates falsified at the root. In that case, the nogood is preprocessed
    ///        to the empty nogood.
    ///     4. Conflicting predicates?
    fn preprocess_nogood(nogood: &mut Vec<Predicate>, context: &mut PropagationContextMut) {
        pumpkin_assert_simple!(context.get_decision_level() == 0);
        // The code below is broken down into several parts

        // We opt for semantic minimisation upfront. This way we avoid the possibility of having
        // assigned predicates in the final nogood. This could happen since the root bound can
        // change since the initial time the semantic minimiser recorded it, so it would not know
        // that a previously nonroot bound is now actually a root bound.

        // Semantic minimisation will take care of removing duplicate predicates, conflicting
        // nogoods, and may result in few predicates since it removes redundancies.
        *nogood = context.semantic_minimiser.minimise(
            nogood,
            context.assignments,
            Mode::EnableEqualityMerging,
        );

        // Check if the nogood cannot be violated, i.e., it has a falsified predicate.
        if nogood.is_empty() || nogood.iter().any(|p| context.is_predicate_falsified(*p)) {
            *nogood = vec![Predicate::trivially_false()];
            return;
        }

        // Remove predicates that are satisfied at the root level.
        nogood.retain(|p| !context.is_predicate_satisfied(*p));

        // If the nogood is violating at the root, the previous retain would leave an empty nogood.
        // Return a violating nogood.
        if nogood.is_empty() {
            *nogood = vec![Predicate::trivially_true()];
        }

        // Done with preprocessing, the result is stored in the input nogood.
    }

    /// Adds a nogood which has been learned during search.
    ///
    /// The first predicate should be asserting and the second predicate should contain the
    /// predicte with the next highest decision level.
    pub(crate) fn add_asserting_nogood(
        &mut self,
        nogood: Vec<Predicate>,
        context: &mut PropagationContextMut,
    ) {
        // We treat unit nogoods in a special way by adding it as a permanent nogood at the
        // root-level; this is essentially the same as adding a predicate at the root level
        if nogood.len() == 1 {
            pumpkin_assert_moderate!(
                context.get_decision_level() == 0,
                "A unit nogood should have backtracked to the root-level"
            );
            self.add_permanent_nogood(nogood, context)
                .expect("Unit learned nogoods cannot fail.");
            return;
        }

        // Skip the zero-th predicate since it is unassigned,
        // but will be assigned at the level of the predicate at index one.
        let lbd = self
            .lbd_helper
            .compute_lbd(&nogood.as_slice()[1..], context.assignments());

        // Add the nogood to the database.
        //
        // If there is an available nogood id, use it, otherwise allocate a fresh id.
        let new_id = if let Some(reused_id) = self.delete_ids.pop() {
            self.nogoods[reused_id] = Nogood::new_learned_nogood(nogood.into(), lbd);
            reused_id
        } else {
            let new_nogood_id = NogoodId {
                id: self.nogoods.len() as u32,
            };
            let _ = self
                .nogoods
                .push(Nogood::new_learned_nogood(nogood.into(), lbd));
            new_nogood_id
        };

        // Now we add two watchers to the first two predicates in the nogood
        self.add_watcher(self.nogoods[new_id].predicates[0], new_id);
        self.add_watcher(self.nogoods[new_id].predicates[1], new_id);

        // Then we propagate the asserting predicate and as reason we give the index to the
        // asserting nogood such that we can re-create the reason when asked for it
        let reason = Reason::DynamicLazy(new_id.id as u64);
        context
            .post_predicate(!self.nogoods[new_id].predicates[0], reason)
            .expect("Cannot fail to add the asserting predicate.");

        // We then divide the new nogood based on the LBD level
        if lbd <= self.parameters.lbd_threshold {
            self.learned_nogood_ids.low_lbd.push(new_id);
        } else {
            self.learned_nogood_ids.high_lbd.push(new_id);
        }
    }

    /// Adds a nogood to the propagator as a permanent nogood and sets the internal state to be
    /// infeasible if the nogood led to a conflict.
    pub(crate) fn add_nogood(
        &mut self,
        nogood: Vec<Predicate>,
        context: &mut PropagationContextMut,
    ) -> Result<(), ConstraintOperationError> {
        match self.add_permanent_nogood(nogood, context) {
            Ok(_) => Ok(()),
            Err(e) => {
                self.is_in_infeasible_state = true;
                Err(e)
            }
        }
    }

    /// Adds a nogood which cannot be deleted by clause management.
    fn add_permanent_nogood(
        &mut self,
        mut nogood: Vec<Predicate>,
        context: &mut PropagationContextMut,
    ) -> Result<(), ConstraintOperationError> {
        pumpkin_assert_simple!(
            context.get_decision_level() == 0,
            "Only allowed to add nogoods permanently at the root for now."
        );

        // If we are already in an infeasible state then we simply return that we are in an
        // infeasible state.
        if self.is_in_infeasible_state {
            return Err(ConstraintOperationError::InfeasibleState);
        }

        // If the nogood is empty then it is automatically satisfied (though it is unusual!)
        if nogood.is_empty() {
            warn!("Adding empty nogood, unusual!");
            return Ok(());
        }

        // Then we pre-process the nogood such that (among others) it does not contain duplicates
        Self::preprocess_nogood(&mut nogood, context);

        // Unit nogoods are added as root assignments rather than as nogoods.
        if nogood.len() == 1 {
            if context.is_predicate_satisfied(nogood[0]) {
                // If the predicate is already satisfied then we report a conflict
                self.is_in_infeasible_state = true;
                Err(ConstraintOperationError::InfeasibleNogood)
            } else if context.is_predicate_falsified(nogood[0]) {
                // If the predicate is already falsified then we don't do anything and simply
                // return success
                Ok(())
            } else {
                // Post the negated predicate at the root to respect the nogood.
                let result = context.post_predicate(!nogood[0], conjunction!());
                match result {
                    Ok(_) => Ok(()),
                    Err(_) => {
                        self.is_in_infeasible_state = true;
                        Err(ConstraintOperationError::InfeasibleNogood)
                    }
                }
            }
        }
        // Standard case, nogood is of size at least two.
        //
        // The preprocessing ensures that all predicates are unassigned.
        else {
            // Add the nogood to the database.
            // If there is an available nogood id, use it, otherwise allocate a fresh id.
            let new_id = if let Some(reused_id) = self.delete_ids.pop() {
                self.nogoods[reused_id] = Nogood::new_permanent_nogood(nogood.into());
                reused_id
            } else {
                self.nogoods
                    .push(Nogood::new_permanent_nogood(nogood.into()))
            };

            self.permanent_nogoods.push(new_id);

            self.add_watcher(self.nogoods[new_id].predicates[0], new_id);
            self.add_watcher(self.nogoods[new_id].predicates[1], new_id);

            Ok(())
        }
    }

    /// Adds a watcher to the predicate in the provided nogood with the provided [`NogoodId`].
    fn add_watcher(&mut self, predicate: Predicate, nogood_id: NogoodId) {
        // First we resize the watch list to accomodate the new nogood
        if predicate.get_domain().id as usize >= self.watch_lists.len() {
            self.watch_lists.resize(
                (predicate.get_domain().id + 1) as usize,
                NogoodWatchList::default(),
            );
        }

        // Then we add this nogood to the watch list of the new watcher.
        match predicate {
            Predicate::LowerBound {
                domain_id,
                lower_bound,
            } => self.watch_lists[domain_id].lower_bound.push(NogoodWatcher {
                right_hand_side: lower_bound,
                nogood_id,
            }),
            Predicate::UpperBound {
                domain_id,
                upper_bound,
            } => self.watch_lists[domain_id].upper_bound.push(NogoodWatcher {
                right_hand_side: upper_bound,
                nogood_id,
            }),
            Predicate::NotEqual {
                domain_id,
                not_equal_constant,
            } => self.watch_lists[domain_id].hole.push(NogoodWatcher {
                right_hand_side: not_equal_constant,
                nogood_id,
            }),
            Predicate::Equal {
                domain_id,
                equality_constant,
            } => self.watch_lists[domain_id].equals.push(NogoodWatcher {
                right_hand_side: equality_constant,
                nogood_id,
            }),
        }
    }

    fn debug_propagate_nogood_from_scratch(
        &self,
        nogood_id: NogoodId,
        context: &mut PropagationContextMut,
    ) -> Result<(), Inconsistency> {
        // This is an inefficient implementation for testing purposes
        let nogood = &self.nogoods[nogood_id];

        // First we get the number of falsified predicates
        let has_falsified_predicate = nogood
            .predicates
            .iter()
            .any(|predicate| context.evaluate_predicate(*predicate).is_some_and(|x| !x));

        // If at least one predicate is false, then the nogood can be skipped
        if has_falsified_predicate {
            return Ok(());
        }

        let num_satisfied_predicates = nogood
            .predicates
            .iter()
            .filter(|predicate| context.evaluate_predicate(**predicate).is_some_and(|x| x))
            .count();

        let nogood_len = nogood.predicates.len();

        // If all predicates in the nogood are satisfied, there is a conflict.
        if num_satisfied_predicates == nogood_len {
            return Err(Inconsistency::Conflict(
                nogood.predicates.iter().copied().collect(),
            ));
        }
        // If all but one predicate are satisfied, then we can propagate.
        //
        // Note that this only makes sense since we know that there are no falsifying predicates at
        // this point.
        else if num_satisfied_predicates == nogood_len - 1 {
            // Note that we negate the remaining unassigned predicate!
            let propagated_predicate = nogood
                .predicates
                .iter()
                .find(|predicate| context.evaluate_predicate(**predicate).is_none())
                .unwrap()
                .not();

            assert!(nogood
                .predicates
                .iter()
                .any(|p| *p == propagated_predicate.not()));

            // Cannot use lazy explanations when propagating from scratch
            // since the propagated predicate may not be at position zero.
            // but we cannot change the nogood since this function is with nonmutable self.
            //
            // So an eager reason is constructed
            let reason: PropositionalConjunction = nogood
                .predicates
                .iter()
                .filter(|p| **p != !propagated_predicate)
                .copied()
                .collect();

            context.post_predicate(propagated_predicate, reason)?;
        }
        Ok(())
    }

    /// Checks for each nogood whether the first two predicates in the nogood are being watched
    fn debug_is_properly_watched(&self) -> bool {
        let is_watching =
            |predicate: Predicate, nogood_id: NogoodId| -> bool {
                match predicate {
                    Predicate::LowerBound {
                        domain_id,
                        lower_bound,
                    } => self.watch_lists[domain_id]
                        .lower_bound
                        .iter()
                        .any(|w| w.right_hand_side == lower_bound && w.nogood_id == nogood_id),
                    Predicate::UpperBound {
                        domain_id,
                        upper_bound,
                    } => self.watch_lists[domain_id]
                        .upper_bound
                        .iter()
                        .any(|w| w.right_hand_side == upper_bound && w.nogood_id == nogood_id),
                    Predicate::NotEqual {
                        domain_id,
                        not_equal_constant,
                    } => self.watch_lists[domain_id].hole.iter().any(|w| {
                        w.right_hand_side == not_equal_constant && w.nogood_id == nogood_id
                    }),
                    Predicate::Equal {
                        domain_id,
                        equality_constant,
                    } => self.watch_lists[domain_id].equals.iter().any(|w| {
                        w.right_hand_side == equality_constant && w.nogood_id == nogood_id
                    }),
                }
            };

        for nogood in self.nogoods.iter().enumerate() {
            let nogood_id = NogoodId {
                id: nogood.0 as u32,
            };

            if !(is_watching(nogood.1.predicates[0], nogood_id)
                && is_watching(nogood.1.predicates[1], nogood_id))
            {
                eprintln!("Nogood id: {}", nogood_id.id);
                eprintln!("Nogood: {:?}", nogood);
                eprintln!(
                    "watching 0: {}",
                    is_watching(nogood.1.predicates[0], nogood_id)
                );
                eprintln!(
                    "watching 1: {}",
                    is_watching(nogood.1.predicates[1], nogood_id)
                );
                eprintln!(
                    "watch list 0: {:?}",
                    self.watch_lists[nogood.1.predicates[0].get_domain()]
                );
                eprintln!(
                    "watch list 1: {:?}",
                    self.watch_lists[nogood.1.predicates[1].get_domain()]
                );
            }

            assert!(
                is_watching(nogood.1.predicates[0], nogood_id)
                    && is_watching(nogood.1.predicates[1], nogood_id)
            );
        }
        true
    }

    /// Removes nogoods if there are too many nogoods with a "high" LBD
    fn clean_up_learned_nogoods_if_needed(&mut self, context: PropagationContext) {
        // Only remove learned nogoods if there are too many.
        if self.learned_nogood_ids.high_lbd.len() > self.parameters.limit_num_high_lbd_nogoods {
            // The procedure is divided into two parts (for simplicity of implementation).
            //  1. Promote nogoods that are in the high lbd group but got updated to a low lbd.
            //  2. Remove roughly half of the nogoods that have high lbd.
            self.promote_high_lbd_nogoods();
            self.remove_high_lbd_nogoods(context);
        }
    }

    /// Goes through all of the "high" LBD nogoods and promotes nogoods which have been updated to
    /// a "low" LBD.
    fn promote_high_lbd_nogoods(&mut self) {
        self.learned_nogood_ids.high_lbd.retain(|id| {
            // If the LBD is still high, the nogood stays in the high LBD category.
            if self.nogoods[*id].lbd > self.parameters.lbd_threshold {
                true
            }
            // Otherwise the nogood is promoted to the low LBD group.
            else {
                self.learned_nogood_ids.low_lbd.push(*id);
                false
            }
        })
    }

    /// Removes the noogd from the watch list
    fn remove_nogood_from_watch_list(
        watch_lists: &mut KeyedVec<DomainId, NogoodWatchList>,
        watching_predicate: Predicate,
        id: NogoodId,
    ) {
        let find_and_remove_watcher = |watch_list: &mut Vec<NogoodWatcher>, value: i32| {
            let position = watch_list
                .iter()
                .position(|w| w.right_hand_side == value && w.nogood_id == id)
                .expect("Watcher must be present.");
            let _ = watch_list.swap_remove(position);
        };

        match watching_predicate {
            Predicate::LowerBound {
                domain_id,
                lower_bound,
            } => (find_and_remove_watcher)(&mut watch_lists[domain_id].lower_bound, lower_bound),
            Predicate::UpperBound {
                domain_id,
                upper_bound,
            } => (find_and_remove_watcher)(&mut watch_lists[domain_id].upper_bound, upper_bound),
            Predicate::NotEqual {
                domain_id,
                not_equal_constant,
            } => (find_and_remove_watcher)(&mut watch_lists[domain_id].hole, not_equal_constant),
            Predicate::Equal {
                domain_id,
                equality_constant,
            } => (find_and_remove_watcher)(&mut watch_lists[domain_id].equals, equality_constant),
        }
    }

    /// Removes high LBD nogoods from the internal structures.
    ///
    /// The idea is that these are likely poor quality nogoods and the overhead of propagating them
    /// is not worth it.
    fn remove_high_lbd_nogoods(&mut self, context: PropagationContext) {
        assert!(
            context.get_decision_level() == 0,
            "We only consider removing high LBD nogoods at the root-level for now"
        );

        // First we sort the high LBD nogoods based on non-increasing "quality"
        self.sort_high_lbd_nogoods_by_quality_better_first();

        // The removal is done in two phases.
        // 1) Nogoods are deleted but the ids are not removed from self.learned_nogoods_ids.
        // 2) The corresponding ids are removed from the self.learned_nogoods_ids.
        let mut num_clauses_to_remove =
            self.learned_nogood_ids.high_lbd.len() - self.parameters.limit_num_high_lbd_nogoods / 2;

        // Note the 'rev', since poor nogoods have priority for deletion.
        // The aim is to remove half of the nogoods, but less could be removed due to protection.
        for &id in self.learned_nogood_ids.high_lbd.iter().rev() {
            if num_clauses_to_remove == 0 {
                // We are not removing any clauses
                break;
            }

            // Protected clauses are skipped for one clean up iteration.
            if self.nogoods[id].is_protected {
                self.nogoods[id].is_protected = false;
                continue;
            }

            // Remove the nogood from the watch list.
            Self::remove_nogood_from_watch_list(
                &mut self.watch_lists,
                self.nogoods[id].predicates[0],
                id,
            );
            Self::remove_nogood_from_watch_list(
                &mut self.watch_lists,
                self.nogoods[id].predicates[1],
                id,
            );

            // Delete the nogood.
            //
            // Note that the deleted nogood is still kept in the database but it will not be used
            // for propagation. A new nogood may take the place of a deleted nogood, this makes it
            // simpler, since other nogood ids remain unchanged.
            self.nogoods[id].is_deleted = true;
            self.delete_ids.push(id);

            num_clauses_to_remove -= 1;
        }

        // Now we remove all of the nogoods from the `high_lbd` nogoods; note that this does not
        // remove it from the database.
        self.learned_nogood_ids
            .high_lbd
            .retain(|&id| !self.nogoods[id].is_deleted);
    }

    /// Orders the `high_lbd` nogoods in such a way that the 'better' nogoods are in front.
    ///
    /// The sorting depends on the provided [`LearnedNogoodSortingStrategy`]
    fn sort_high_lbd_nogoods_by_quality_better_first(&mut self) {
        // Note that this is not the most efficient sorting comparison, but will do for now.
        self.learned_nogood_ids
            .high_lbd
            .sort_unstable_by(|&id1, &id2| {
                let nogood1 = &self.nogoods[id1];
                let nogood2 = &self.nogoods[id2];

                match self.parameters.nogood_sorting_strategy {
                    LearnedNogoodSortingStrategy::Activity => {
                        // Note that here we reverse nogood1 and nogood2,
                        // because a higher value for activity is better.
                        nogood2.activity.partial_cmp(&nogood1.activity).unwrap()
                    }
                    LearnedNogoodSortingStrategy::Lbd => {
                        if nogood1.lbd != nogood2.lbd {
                            // Recall that lower LBD is better.
                            nogood1.lbd.cmp(&nogood2.lbd)
                        } else {
                            // Note that here we reverse nogood1 and nogood2,
                            // because a higher value for activity is better.
                            nogood2.activity.partial_cmp(&nogood1.activity).unwrap()
                        }
                    }
                }
            });
    }

    /// Decays the activity bump increment by
    /// [`LearningOptions::self.parameters.activity_decay_factor`].
    pub(crate) fn decay_nogood_activities(&mut self) {
        self.activity_bump_increment /= self.parameters.activity_decay_factor;
        for &id in &self.bumped_nogoods {
            self.nogoods[id].block_bumps = false;
        }
        self.bumped_nogoods.clear();
    }

    /// Similar to [`NogoodPropagator::add_watcher`] but with different input parameters to avoid
    /// issues with borrow checks and handles the special case with holes in the domain.
    ///
    /// Special case with holes in the domain:
    /// In the case that a watcher is going to replace the current watcher (due to it now being
    /// satisfied) and the following two conditions hold:
    ///     1. It has a predicate with the same [`DomainId`]
    ///     2. It is also a not-equals predicate
    /// Then the current watcher should not be removed from the list but instead only its
    /// right-hand side should be updated; this is stored in `kept_watcher_new_rhs`
    fn add_new_nogood_watcher(
        watch_lists: &mut KeyedVec<DomainId, NogoodWatchList>,
        predicate: Predicate,
        nogood_id: NogoodId,
        domain_event: IntDomainEvent,
        updated_domain_id: DomainId,
        kept_watcher_new_rhs: &mut Option<i32>,
    ) {
        // Add this nogood to the watch list of the new watcher.
        match predicate {
            Predicate::LowerBound {
                domain_id,
                lower_bound,
            } => watch_lists[domain_id].lower_bound.push(NogoodWatcher {
                right_hand_side: lower_bound,
                nogood_id,
            }),
            Predicate::UpperBound {
                domain_id,
                upper_bound,
            } => watch_lists[domain_id].upper_bound.push(NogoodWatcher {
                right_hand_side: upper_bound,
                nogood_id,
            }),
            Predicate::NotEqual {
                domain_id,
                not_equal_constant,
            } => {
                if let IntDomainEvent::Removal = domain_event {
                    if domain_id != updated_domain_id {
                        // The domain ids of the watchers are not the same, we default to the
                        // regular case and simply replace the watcher
                        watch_lists[domain_id].hole.push(NogoodWatcher {
                            right_hand_side: not_equal_constant,
                            nogood_id,
                        })
                    } else {
                        // The watcher should stay in this list, but change
                        // its right hand side to reflect the new watching
                        // predicate
                        //
                        // Here we only note that the watcher should stay, and later it actually
                        // gets copied.
                        *kept_watcher_new_rhs = Some(not_equal_constant);
                    }
                } else {
                    watch_lists[domain_id].hole.push(NogoodWatcher {
                        right_hand_side: not_equal_constant,
                        nogood_id,
                    })
                }
            }
            Predicate::Equal {
                domain_id,
                equality_constant,
            } => watch_lists[domain_id].equals.push(NogoodWatcher {
                right_hand_side: equality_constant,
                nogood_id,
            }),
        }
    }

    fn propagate_or_find_new_watcher(
        nogoods: &mut KeyedVec<NogoodId, Nogood>,
        domain_event: IntDomainEvent,
        last_index_on_trail: usize,
        watch_lists: &mut KeyedVec<DomainId, NogoodWatchList>,
        context: &mut PropagationContextMut<'_>,
        updated_domain_id: DomainId,
    ) -> Result<(), Inconsistency> {
        // A helper function for getting the right watch-list for a [`DomainId`] based on the
        // provided [`IntDomainEvent`]
        fn get_watch_list(
            domain_id: DomainId,
            domain_event: IntDomainEvent,
            watch_lists: &mut KeyedVec<DomainId, NogoodWatchList>,
        ) -> &mut Vec<NogoodWatcher> {
            match domain_event {
                IntDomainEvent::Assign => &mut watch_lists[domain_id].equals,
                IntDomainEvent::LowerBound => &mut watch_lists[domain_id].lower_bound,
                IntDomainEvent::UpperBound => &mut watch_lists[domain_id].upper_bound,
                IntDomainEvent::Removal => &mut watch_lists[domain_id].hole,
            }
        }

        // A helper function for determining whether an update of the watched predicate has taken
        // place

        fn has_been_updated(
            domain_event: IntDomainEvent,
            right_hand_side: i32,
            context: &PropagationContext,
            updated_domain_id: DomainId,
            last_index_on_trail: usize,
        ) -> bool {
            // First we get the values for checking whether or not the predicate was previously not
            // satisfied and now is
            let (old_lower_bound, new_lower_bound, old_upper_bound, new_upper_bound) =
                match domain_event {
                    IntDomainEvent::LowerBound => (
                        context
                            .lower_bound_at_trail_position(&updated_domain_id, last_index_on_trail),
                        context.lower_bound(&updated_domain_id),
                        0,
                        0,
                    ),
                    IntDomainEvent::UpperBound => (
                        0,
                        0,
                        context
                            .upper_bound_at_trail_position(&updated_domain_id, last_index_on_trail),
                        context.upper_bound(&updated_domain_id),
                    ),
                    IntDomainEvent::Removal | IntDomainEvent::Assign => (
                        context
                            .lower_bound_at_trail_position(&updated_domain_id, last_index_on_trail),
                        context.lower_bound(&updated_domain_id),
                        context
                            .upper_bound_at_trail_position(&updated_domain_id, last_index_on_trail),
                        context.upper_bound(&updated_domain_id),
                    ),
                };
            match domain_event {
                IntDomainEvent::Assign => {
                    // We perform a simple check that the new bounds are the same and that it is
                    // equal to the right-hand side
                    pumpkin_assert_simple!(new_lower_bound == new_upper_bound);
                    right_hand_side == new_lower_bound
                }
                IntDomainEvent::LowerBound => {
                    // We check whether the previous lower-bound is smaller than the right-hand
                    // side but the new lower-bound is larger than the right-hand side
                    old_lower_bound < right_hand_side && right_hand_side <= new_lower_bound
                }
                IntDomainEvent::UpperBound => {
                    // We check whether the previous upper-bound is larger than the right-hand side
                    // but the new upper-bound is smaller than the right-hand side
                    old_upper_bound > right_hand_side && right_hand_side >= new_upper_bound
                }
                IntDomainEvent::Removal => {
                    // A more involved check, we look at the watcher if:
                    //      1) The removed value was definitely removed due to a bound change
                    //      2) The removed value is within the bounds, and was actually removed

                    // The first condition checks whether the upper-bound used to be larger than the
                    // right-hand side (i.e. the right-hand side was within the upper-bound) and
                    // now it is not
                    let value_removed_by_upper_bound_change =
                        old_upper_bound >= right_hand_side && right_hand_side > new_upper_bound;
                    // The second condition checks whether the lower-bound used to be smaller than
                    // the right-hand side (i.e. the right-hand side was within the lower-bound)
                    // and now it is not
                    let value_removed_by_lower_bound_change =
                        old_lower_bound <= right_hand_side && right_hand_side < new_lower_bound;
                    // The third condition checks whether the right-hand is within the new
                    // lower-bound and upper-bound but whether the value is explicitly not in the
                    // domain
                    let value_explicitly_removed = new_lower_bound < right_hand_side
                        && right_hand_side < new_lower_bound
                        && context.is_predicate_satisfied(predicate!(
                            updated_domain_id != right_hand_side
                        ));
                    value_removed_by_upper_bound_change
                        || value_removed_by_lower_bound_change
                        || value_explicitly_removed
                }
            }
        }

        let mut current_index = 0;
        let mut end_index = 0;

        let num_watchers = get_watch_list(updated_domain_id, domain_event, watch_lists).len();

        // We go through all of the watchers for the watch list of the provided domain event (e.g.
        // if the event was a lower-bound event then we only go through the lower-bound watchers
        // since these are the only ones which should be updated)
        while current_index < num_watchers {
            // We retrieve the value from the watcher
            let NogoodWatcher {
                right_hand_side,
                nogood_id,
            } = get_watch_list(updated_domain_id, domain_event, watch_lists)[current_index];

            // Then we check whether the watcher has been updated since the last time that we
            // checked
            if has_been_updated(
                domain_event,
                right_hand_side,
                &context.as_readonly(),
                updated_domain_id,
                last_index_on_trail,
            ) {
                // TODO: check cached predicate?

                // If the watcher has been updated then we need to either propagate or find a
                // replacement watcher

                // First we retrieve the nogood
                let nogood = &mut nogoods[nogood_id].predicates;

                // A helper which checks whether a predicate is the one for which the update has
                // occurred
                let is_watched_predicate = |predicate: Predicate| {
                    // First we perform a check whether the predicate is indeed the right-type and
                    // whether some additional conditions hold
                    let is_matching_predicate = match domain_event {
                        IntDomainEvent::Assign => {
                            predicate.is_equality_predicate()
                                && right_hand_side == context.lower_bound(&updated_domain_id)
                        }
                        IntDomainEvent::LowerBound => predicate.is_lower_bound_predicate(),
                        IntDomainEvent::UpperBound => predicate.is_upper_bound_predicate(),
                        IntDomainEvent::Removal => {
                            predicate.is_not_equal_predicate()
                                && predicate.get_right_hand_side() == right_hand_side
                        }
                    };

                    // Then we return whether the predicate matches the watcher and whether the
                    // domain of the predicate is indeed the domain id which has been updated
                    is_matching_predicate && predicate.get_domain() == updated_domain_id
                };

                // Place the watched predicate at position 1 for simplicity.
                if is_watched_predicate(nogood[0]) {
                    nogood.swap(0, 1);
                }

                // At this point, we have detected that the watcher predicate is
                // satisfied so this predicate should also hold
                pumpkin_assert_moderate!(context.is_predicate_satisfied(nogood[1]));

                // Check the other watched predicate is already falsified, in which case
                // no propagation can take place.
                //
                // Recall that the other watched predicate is at position 0 due to previous code.
                //
                // TODO: check if comparing to the cache literal would make sense.
                if context.is_predicate_falsified(nogood[0]) {
                    // Keep the watchers, the nogood is falsified,
                    //
                    // No propagation can take place.
                    get_watch_list(updated_domain_id, domain_event, watch_lists)[end_index] =
                        get_watch_list(updated_domain_id, domain_event, watch_lists)[current_index];
                    current_index += 1;
                    end_index += 1;
                    continue;
                }

                // Look for another nonsatisfied predicate to replace the watched predicate.
                let mut found_new_watch = false;
                // This value is used to keep track of the special case for holes in the domain
                let mut kept_watcher_new_rhs: Option<i32> = None;

                // Start from index 2 since we are skipping watched predicates.
                for i in 2..nogood.len() {
                    // Find a predicate that is either false or unassigned,
                    // i.e., not assigned true.
                    if !context.is_predicate_satisfied(nogood[i]) {
                        // Found another predicate that can be the watcher.
                        found_new_watch = true;
                        // TODO: does it make sense to replace the cached predicate with
                        // this new predicate?

                        // Replace the current watcher with the new predicate watcher.
                        nogood.swap(1, i);
                        pumpkin_assert_moderate!(nogood[i].get_domain() == updated_domain_id);
                        NogoodPropagator::add_new_nogood_watcher(
                            watch_lists,
                            nogood[1],
                            nogood_id,
                            domain_event,
                            updated_domain_id,
                            &mut kept_watcher_new_rhs,
                        );

                        // No propagation is taking place, go to the next nogood.
                        break;
                    }
                }

                // If we have found a replacement watcher then we either replace it or update it
                // appropriately and continue
                if found_new_watch {
                    if let Some(new_rhs) = kept_watcher_new_rhs {
                        // Keep the current watch for this predicate,
                        // and update its right hand side.
                        get_watch_list(updated_domain_id, domain_event, watch_lists)[end_index] =
                            get_watch_list(updated_domain_id, domain_event, watch_lists)
                                [current_index];
                        get_watch_list(updated_domain_id, domain_event, watch_lists)[end_index]
                            .right_hand_side = new_rhs;

                        end_index += 1;
                        current_index += 1;

                        continue;
                    } else {
                        // Note this nogood is effectively removed from the watch list
                        // of the the current predicate, since we
                        // are only incrementing the current index, and not copying
                        // anything to the end_index.
                        current_index += 1;
                        continue;
                    }
                }

                // We have not found a replacement watcher and we should propagate now

                // Keep the current watch for this predicate.
                get_watch_list(updated_domain_id, domain_event, watch_lists)[end_index] =
                    get_watch_list(updated_domain_id, domain_event, watch_lists)[current_index];
                end_index += 1;
                current_index += 1;

                // At this point, nonwatched predicates and nogood[1] are falsified.
                pumpkin_assert_advanced!(nogood
                    .iter()
                    .skip(1)
                    .all(|p| context.is_predicate_satisfied(*p)));

                // There are two scenarios:
                //      1) nogood[0] is unassigned -> propagate the predicate to false
                //      2) nogood[0] is assigned true -> conflict.
                let reason = Reason::DynamicLazy(nogood_id.id as u64);

                let result = context.post_predicate(!nogood[0], reason);
                // If the propagation lead to a conflict.
                if let Err(e) = result {
                    // Stop any further propagation and report the conflict.
                    // Readd the remaining watchers to the watch list.
                    while current_index < num_watchers {
                        get_watch_list(updated_domain_id, domain_event, watch_lists)[end_index] =
                            get_watch_list(updated_domain_id, domain_event, watch_lists)
                                [current_index];
                        current_index += 1;
                        end_index += 1;
                    }
                    get_watch_list(updated_domain_id, domain_event, watch_lists)
                        .truncate(end_index);
                    return Err(e.into());
                }
            } else {
                // If no update has taken place then we simply keep the current watch for this
                // predicate.
                get_watch_list(updated_domain_id, domain_event, watch_lists)[end_index] =
                    get_watch_list(updated_domain_id, domain_event, watch_lists)[current_index];
                end_index += 1;
                current_index += 1;
            }
        }

        // We have traversed all of the watchers
        if num_watchers > 0 {
            get_watch_list(updated_domain_id, domain_event, watch_lists).truncate(end_index);
        }
        Ok(())
    }
}

impl Propagator for NogoodPropagator {
    fn name(&self) -> &str {
        // It is important to keep this name exactly this.
        // In parts of code for debugging, it looks for this particular name.
        "NogoodPropagator"
    }

    fn priority(&self) -> u32 {
        0
    }

    fn propagate(&mut self, mut context: PropagationContextMut) -> Result<(), Inconsistency> {
        pumpkin_assert_advanced!(self.debug_is_properly_watched());

        if self.watch_lists.len() <= context.assignments().num_domains() as usize {
            self.watch_lists.resize(
                context.assignments().num_domains() as usize + 1,
                NogoodWatchList::default(),
            );
        }

        let old_trail_position = context.assignments.trail.len() - 1;

        // Because drain lazily removes and updates internal data structures, in case a conflict is
        // detected and the loop exits, some elements might not get cleaned up properly.
        //
        // So we eager call each elements here by copying. Could think about avoiding this in the
        // future.
        let events: Vec<(IntDomainEvent, DomainId)> = self.enqueued_updates.drain().collect();

        // We go over all of the events which we have been notified of to determine whether the
        // watchers should be updated or whether a propagation can take place
        for (update_event, updated_domain_id) in events {
            NogoodPropagator::propagate_or_find_new_watcher(
                &mut self.nogoods,
                update_event,
                self.last_index_on_trail,
                &mut self.watch_lists,
                &mut context,
                updated_domain_id,
            )?;
        }
        self.last_index_on_trail = old_trail_position;

        pumpkin_assert_advanced!(self.debug_is_properly_watched());

        Ok(())
    }

    fn synchronise(&mut self, context: PropagationContext) {
        self.last_index_on_trail = context.assignments().trail.len() - 1;
        let _ = self.enqueued_updates.drain();

        if context.assignments.get_decision_level() == 0 {
            self.clean_up_learned_nogoods_if_needed(context);
        }
    }

    fn notify(
        &mut self,
        _context: PropagationContext,
        local_id: LocalId,
        event: OpaqueDomainEvent,
    ) -> EnqueueDecision {
        while local_id.unpack() as usize >= self.enqueued_updates.num_domains() {
            self.enqueued_updates.grow();
        }

        // Save the update, and also enqueue removal in case the lower or upper bound updates are
        // set.
        self.enqueued_updates.event_occurred(
            event.unwrap(),
            DomainId {
                id: local_id.unpack(),
            },
        );
        if let IntDomainEvent::LowerBound | IntDomainEvent::UpperBound = event.unwrap() {
            // If it is a lower-bound or upper-bound event then we also add a removal event
            self.enqueued_updates.event_occurred(
                IntDomainEvent::Removal,
                DomainId {
                    id: local_id.unpack(),
                },
            );
        }
        EnqueueDecision::Enqueue
    }

    fn debug_propagate_from_scratch(
        &self,
        mut context: PropagationContextMut,
    ) -> Result<(), Inconsistency> {
        // Very inefficient version!

        // The algorithm goes through every nogood explicitly
        // and computes from scratch.
        for nogood_id in self.nogoods.keys() {
            self.debug_propagate_nogood_from_scratch(nogood_id, &mut context)?;
        }
        Ok(())
    }

    /// Returns the slice representing a conjunction of predicates that explain the propagation
    /// encoded by the code, which was given to the solver by the propagator at the time of
    /// propagation.
    ///
    /// In case of the noogood propagator, lazy explanations internally also update information
    /// about the LBD and activity of the nogood, which is used when cleaning up nogoods.
    fn lazy_explanation(&mut self, code: u64, assignments: &Assignments) -> &[Predicate] {
        let id = NogoodId { id: code as u32 };
        // Update the LBD and activity of the nogood, if appropriate.
        //
        // Note that low lbd nogoods are kept permanently, so these are not updated.
        if !self.nogoods[id].block_bumps
            && self.nogoods[id].is_learned
            && self.nogoods[id].lbd > self.parameters.lbd_threshold
        {
            self.nogoods[id].block_bumps = true;
            self.bumped_nogoods.push(id);
            // LBD update.
            // Note that we do not need to take into account the propagated predicate (in position
            // zero), since it will share a decision level with one of the other predicates.
            let current_lbd = self
                .lbd_helper
                .compute_lbd(&self.nogoods[id].predicates.as_slice()[1..], assignments);

            // The nogood keeps track of the best lbd encountered.
            if current_lbd < self.nogoods[id].lbd {
                self.nogoods[id].lbd = current_lbd;
                if current_lbd <= 30 {
                    self.nogoods[id].is_protected = true;
                }
            }

            // Nogood activity update.
            // Rescale the nogood activities,
            // in case bumping the activity now would lead to a large activity value.
            if self.nogoods[id].activity + self.activity_bump_increment
                > self.parameters.max_activity
            {
                self.learned_nogood_ids.high_lbd.iter().for_each(|i| {
                    self.nogoods[*i].activity /= self.parameters.max_activity;
                });
                self.activity_bump_increment /= self.parameters.max_activity;
            }

            // At this point, it is safe to increase the activity value
            self.nogoods[id].activity += self.activity_bump_increment;
        }
        // update LBD, so we need code plus assignments as input.
        &self.nogoods[id].predicates.as_slice()[1..]
    }

    fn initialise_at_root(
        &mut self,
        _context: &mut PropagatorInitialisationContext,
    ) -> Result<(), PropositionalConjunction> {
        // There should be no nogoods yet
        pumpkin_assert_simple!(self.nogoods.len() == 0);
        Ok(())
    }
}

/// The watch list is specific to a domain id.
#[derive(Default, Clone, Debug)]
struct NogoodWatchList {
    /// Nogoods with a watched predicate [x >= k]
    lower_bound: Vec<NogoodWatcher>,
    /// Nogoods with a watched predicate [x <= k]
    upper_bound: Vec<NogoodWatcher>,
    /// Nogoods with a watched predicate [x != k]
    hole: Vec<NogoodWatcher>,
    /// Nogoods with a watched predicate [x == k]
    equals: Vec<NogoodWatcher>,
}

/// The watcher is with respect to a specific domain id and predicate type.
#[derive(Default, Clone, Copy, Debug)]
struct NogoodWatcher {
    /// This field represents the right-hand side of the predicate present in the nogood.
    ///
    /// It is used as an indicator to whether the nogood should be inspected.
    right_hand_side: i32,
    nogood_id: NogoodId,
    // todo: consider the cached literal
}

#[derive(Default, Clone, Copy, Debug, PartialEq)]
struct NogoodId {
    id: u32,
}

impl StorageKey for NogoodId {
    fn index(&self) -> usize {
        self.id as usize
    }

    fn create_from_index(index: usize) -> Self {
        NogoodId { id: index as u32 }
    }
}

#[cfg(test)]
mod tests {
    use super::NogoodPropagator;
    use crate::conjunction;
    use crate::engine::propagation::store::PropagatorStore;
    use crate::engine::propagation::PropagationContextMut;
    use crate::engine::propagation::PropagatorId;
    use crate::engine::test_solver::TestSolver;
    use crate::predicate;

    fn downcast_to_nogood_propagator(
        nogood_propagator: PropagatorId,
        propagators: &mut PropagatorStore,
    ) -> &mut NogoodPropagator {
        match propagators[nogood_propagator].downcast_mut::<NogoodPropagator>() {
            Some(nogood_propagator) => nogood_propagator,
            None => panic!("Provided propagator should be the nogood propagator"),
        }
    }

    #[test]
    fn ternary_nogood_propagate() {
        let mut solver = TestSolver::default();
        let dummy = solver.new_variable(0, 1);
        let a = solver.new_variable(1, 3);
        let b = solver.new_variable(-4, 4);
        let c = solver.new_variable(-10, 20);

        let propagator = solver
            .new_propagator(NogoodPropagator::default())
            .expect("no empty domains");

        let _ = solver.increase_lower_bound_and_notify(propagator, dummy.id, dummy, 1);

        let nogood = conjunction!([a >= 2] & [b >= 1] & [c >= 10]);
        {
            let mut context = PropagationContextMut::new(
                &mut solver.assignments,
                &mut solver.reason_store,
                &mut solver.semantic_minimiser,
                propagator,
            );

            downcast_to_nogood_propagator(propagator, &mut solver.propagator_store)
                .add_nogood(nogood.into(), &mut context)
                .expect("");
        }

        let _ = solver.increase_lower_bound_and_notify(propagator, a.id, a, 3);
        let _ = solver.increase_lower_bound_and_notify(propagator, b.id, b, 0);

        solver.propagate_until_fixed_point(propagator).expect("");

        let _ = solver.increase_lower_bound_and_notify(propagator, c.id, c, 15);

        solver.propagate(propagator).expect("");

        assert_eq!(solver.upper_bound(b), 0);

        let reason_lb = solver.get_reason_int(predicate!(b <= 0));
        assert_eq!(conjunction!([a >= 2] & [c >= 10]).as_slice(), reason_lb);
    }

    #[test]
    fn unsat() {
        let mut solver = TestSolver::default();
        let a = solver.new_variable(1, 3);
        let b = solver.new_variable(-4, 4);
        let c = solver.new_variable(-10, 20);

        let propagator = solver
            .new_propagator(NogoodPropagator::default())
            .expect("no empty domains");

        let nogood = conjunction!([a >= 2] & [b >= 1] & [c >= 10]);
        {
            let mut context = PropagationContextMut::new(
                &mut solver.assignments,
                &mut solver.reason_store,
                &mut solver.semantic_minimiser,
                propagator,
            );

            downcast_to_nogood_propagator(propagator, &mut solver.propagator_store)
                .add_nogood(nogood.into(), &mut context)
                .expect("");
        }

        let _ = solver.increase_lower_bound_and_notify(propagator, a.id, a, 3);
        let _ = solver.increase_lower_bound_and_notify(propagator, b.id, b, 1);
        let _ = solver.increase_lower_bound_and_notify(propagator, c.id, c, 15);

        let result = solver.propagate_until_fixed_point(propagator);
        assert!(result.is_err());
    }
}
