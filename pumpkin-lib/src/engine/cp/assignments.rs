use crate::basic_types::HashMap;
use crate::basic_types::KeyedVec;
use crate::basic_types::Trail;
use crate::engine::cp::event_sink::EventSink;
use crate::engine::cp::reason::ReasonRef;
use crate::engine::cp::IntDomainEvent;
use crate::engine::predicates::predicate::Predicate;
use crate::engine::variables::DomainGeneratorIterator;
use crate::engine::variables::DomainId;
use crate::predicate;
use crate::pumpkin_assert_moderate;
use crate::pumpkin_assert_simple;

#[derive(Clone, Debug)]
pub struct Assignments {
    pub(crate) trail: Trail<ConstraintProgrammingTrailEntry>,
    domains: KeyedVec<DomainId, IntegerDomain>,
    events: EventSink,
}

impl Default for Assignments {
    fn default() -> Self {
        let mut assignments = Self {
            trail: Default::default(),
            domains: Default::default(),
            events: Default::default(),
        };
        // As a convention, we allocate a dummy domain_id=0, which represents a 0-1 variable that is
        // assigned to one. We use it to represent predicates that are trivially true.
        let dummy_variable = assignments.grow(1, 1);
        assert!(dummy_variable.id == 0);
        assignments
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EmptyDomain;

impl Assignments {
    pub(crate) fn increase_decision_level(&mut self) {
        self.trail.increase_decision_level()
    }

    pub(crate) fn get_decision_level(&self) -> usize {
        self.trail.get_decision_level()
    }

    pub(crate) fn num_domains(&self) -> u32 {
        self.domains.len() as u32
    }

    pub(crate) fn get_domains(&self) -> DomainGeneratorIterator {
        // todo: we use 1 here to prevent the always true literal from ending up in the blocking
        // clause
        DomainGeneratorIterator::new(1, self.num_domains())
    }

    pub(crate) fn num_trail_entries(&self) -> usize {
        self.trail.len()
    }

    pub(crate) fn get_trail_entry(&self, index: usize) -> ConstraintProgrammingTrailEntry {
        self.trail[index]
    }

    pub(crate) fn get_last_entry_on_trail(&self) -> ConstraintProgrammingTrailEntry {
        *self.trail.last().unwrap()
    }

    #[allow(dead_code)]
    pub(crate) fn get_last_predicates_on_trail(
        &self,
        num_predicates: usize,
    ) -> impl Iterator<Item = Predicate> + '_ {
        self.trail[(self.num_trail_entries() - num_predicates)..self.num_trail_entries()]
            .iter()
            .map(|e| e.predicate)
    }

    #[allow(dead_code)]
    pub(crate) fn get_last_entries_on_trail(
        &self,
        num_predicates: usize,
    ) -> &[ConstraintProgrammingTrailEntry] {
        &self.trail[(self.num_trail_entries() - num_predicates)..self.num_trail_entries()]
    }

    // registers the domain of a new integer variable
    // note that this is an internal method that does _not_ allocate additional information
    // necessary for the solver apart from the domain when creating a new integer variable, use
    // create_new_domain_id in the ConstraintSatisfactionSolver
    pub(crate) fn grow(&mut self, lower_bound: i32, upper_bound: i32) -> DomainId {
        let id = DomainId {
            id: self.num_domains(),
        };

        self.domains
            .push(IntegerDomain::new(lower_bound, upper_bound, id));

        self.events.grow();

        id
    }

    pub(crate) fn drain_domain_events(
        &mut self,
    ) -> impl Iterator<Item = (IntDomainEvent, DomainId)> + '_ {
        self.events.drain()
    }

    pub(crate) fn debug_create_empty_clone(&self) -> Self {
        let mut domains = self.domains.clone();
        let event_sink = EventSink::new(domains.len());
        self.trail.iter().rev().for_each(|entry| {
            domains[entry.predicate.get_domain()].undo_trail_entry(entry);
        });
        Assignments {
            trail: Default::default(),
            domains,
            events: event_sink,
        }
    }
}

// methods for getting info about the domains
impl Assignments {
    pub(crate) fn get_lower_bound(&self, domain_id: DomainId) -> i32 {
        self.domains[domain_id].lower_bound()
    }

    pub(crate) fn get_lower_bound_at_trail_position(
        &self,
        domain_id: DomainId,
        trail_position: usize,
    ) -> i32 {
        self.domains[domain_id].lower_bound_at_trail_position(trail_position)
    }

    pub(crate) fn get_upper_bound(&self, domain_id: DomainId) -> i32 {
        self.domains[domain_id].upper_bound()
    }

    pub(crate) fn get_upper_bound_at_trail_position(
        &self,
        domain_id: DomainId,
        trail_position: usize,
    ) -> i32 {
        self.domains[domain_id].upper_bound_at_trail_position(trail_position)
    }

    pub(crate) fn get_initial_lower_bound(&self, domain_id: DomainId) -> i32 {
        self.domains[domain_id].initial_lower_bound()
    }

    pub(crate) fn get_initial_upper_bound(&self, domain_id: DomainId) -> i32 {
        self.domains[domain_id].initial_upper_bound()
    }

    pub(crate) fn get_initial_holes(&self, domain_id: DomainId) -> Vec<i32> {
        self.domains[domain_id]
            .hole_updates
            .iter()
            .take_while(|h| h.decision_level == 0)
            .map(|h| h.removed_value)
            .collect()
    }

    pub(crate) fn get_assigned_value(&self, domain_id: DomainId) -> Option<i32> {
        if self.is_domain_assigned(domain_id) {
            Some(self.domains[domain_id].lower_bound())
        } else {
            None
        }
    }

    pub(crate) fn get_assigned_value_at_trail_position(
        &self,
        domain_id: DomainId,
        trail_position: usize,
    ) -> Option<i32> {
        if self.is_domain_assigned_at_trail_position(domain_id, trail_position) {
            Some(self.domains[domain_id].lower_bound_at_trail_position(trail_position))
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub(crate) fn get_domain_iterator(&self, domain_id: DomainId) -> IntegerDomainIterator {
        self.domains[domain_id].domain_iterator()
    }

    pub(crate) fn get_domain_description(&self, domain_id: DomainId) -> Vec<Predicate> {
        let mut predicates = Vec::new();
        let domain = &self.domains[domain_id];
        // if fixed, this is just one predicate
        if domain.lower_bound() == domain.upper_bound() {
            predicates.push(predicate![domain_id == domain.lower_bound()]);
            return predicates;
        }
        // if not fixed, start with the bounds...
        predicates.push(predicate![domain_id >= domain.lower_bound()]);
        predicates.push(predicate![domain_id <= domain.upper_bound()]);
        // then the holes...
        for hole in &self.domains[domain_id].holes {
            // Only record holes that are within the lower and upper bound.
            // Note that the bound values cannot be in the holes,
            // so we can use strictly lower/greater than comparison
            if domain.lower_bound() < *hole.0 && *hole.0 < domain.upper_bound() {
                predicates.push(predicate![domain_id != *hole.0]);
            }
        }
        predicates
    }

    pub(crate) fn is_value_in_domain(&self, domain_id: DomainId, value: i32) -> bool {
        let domain = &self.domains[domain_id];
        domain.contains(value)
    }

    pub(crate) fn is_value_in_domain_at_trail_position(
        &self,
        domain_id: DomainId,
        value: i32,
        trail_position: usize,
    ) -> bool {
        self.domains[domain_id].contains_at_trail_position(value, trail_position)
    }

    pub(crate) fn is_domain_assigned(&self, domain_id: DomainId) -> bool {
        self.get_lower_bound(domain_id) == self.get_upper_bound(domain_id)
    }

    pub(crate) fn is_domain_assigned_at_trail_position(
        &self,
        domain_id: DomainId,
        trail_position: usize,
    ) -> bool {
        self.get_lower_bound_at_trail_position(domain_id, trail_position)
            == self.get_upper_bound_at_trail_position(domain_id, trail_position)
    }

    pub(crate) fn is_domain_assigned_to_value(&self, domain_id: DomainId, value: i32) -> bool {
        self.is_domain_assigned(domain_id) && self.get_lower_bound(domain_id) == value
    }

    /// Returns the index of the trail entry at which point the given predicate became true.
    /// In case the predicate is not true, then the function returns None.
    /// Note that it is not necessary for the predicate to be explicitly present on the trail,
    /// e.g., if [x >= 10] is explicitly present on the trail but not [x >= 6], then the
    /// trail position for [x >= 10] will be returned for the case [x >= 6].
    pub(crate) fn get_trail_position(&self, predicate: &Predicate) -> Option<usize> {
        self.domains[predicate.get_domain()]
            .get_update_info(predicate)
            .map(|u| u.trail_position)
    }

    pub(crate) fn get_decision_level_for_predicate(&self, predicate: &Predicate) -> Option<usize> {
        // println!(
        // "{} {} {:?}",
        // predicate,
        // predicate.get_domain(),
        // self.domains[predicate.get_domain()].upper_bound_updates
        // );
        //
        // let m = self.domains[predicate.get_domain()]
        // .get_update_info(predicate)
        // .map(|u| u.decision_level)
        // .unwrap();
        // println!("RET VAL {}", m);

        self.domains[predicate.get_domain()]
            .get_update_info(predicate)
            .map(|u| u.decision_level)
    }
}

// methods to change the domains
impl Assignments {
    pub(crate) fn tighten_lower_bound(
        &mut self,
        domain_id: DomainId,
        new_lower_bound: i32,
        reason: Option<ReasonRef>,
    ) -> Result<(), EmptyDomain> {
        // No need to do any changes if the new lower bound is weaker.
        if new_lower_bound <= self.get_lower_bound(domain_id) {
            return self.domains[domain_id].verify_consistency();
        }

        let predicate = Predicate::LowerBound {
            domain_id,
            lower_bound: new_lower_bound,
        };

        let old_lower_bound = self.get_lower_bound(domain_id);
        let old_upper_bound = self.get_upper_bound(domain_id);

        // important to record trail position _before_ pushing to the trail
        let trail_position = self.trail.len();

        self.trail.push(ConstraintProgrammingTrailEntry {
            predicate,
            old_lower_bound,
            old_upper_bound,
            reason,
        });

        let decision_level = self.get_decision_level();
        let domain = &mut self.domains[domain_id];

        domain.set_lower_bound(
            new_lower_bound,
            decision_level,
            trail_position,
            &mut self.events,
        );

        domain.verify_consistency()
    }

    pub(crate) fn tighten_upper_bound(
        &mut self,
        domain_id: DomainId,
        new_upper_bound: i32,
        reason: Option<ReasonRef>,
    ) -> Result<(), EmptyDomain> {
        // No need to do any changes if the new upper bound is weaker.
        if new_upper_bound >= self.get_upper_bound(domain_id) {
            return self.domains[domain_id].verify_consistency();
        }

        let predicate = Predicate::UpperBound {
            domain_id,
            upper_bound: new_upper_bound,
        };

        let old_lower_bound = self.get_lower_bound(domain_id);
        let old_upper_bound = self.get_upper_bound(domain_id);

        // important to record trail position _before_ pushing to the trail
        let trail_position = self.trail.len();

        self.trail.push(ConstraintProgrammingTrailEntry {
            predicate,
            old_lower_bound,
            old_upper_bound,
            reason,
        });

        let decision_level = self.get_decision_level();
        let domain = &mut self.domains[domain_id];

        domain.set_upper_bound(
            new_upper_bound,
            decision_level,
            trail_position,
            &mut self.events,
        );

        domain.verify_consistency()
    }

    pub(crate) fn make_assignment(
        &mut self,
        domain_id: DomainId,
        assigned_value: i32,
        reason: Option<ReasonRef>,
    ) -> Result<(), EmptyDomain> {
        pumpkin_assert_moderate!(!self.is_domain_assigned_to_value(domain_id, assigned_value));

        // only tighten the lower bound if needed
        if self.get_lower_bound(domain_id) < assigned_value {
            self.tighten_lower_bound(domain_id, assigned_value, reason)?;
        }

        // only tighten the uper bound if needed
        if self.get_upper_bound(domain_id) > assigned_value {
            self.tighten_upper_bound(domain_id, assigned_value, reason)?;
        }

        self.domains[domain_id].verify_consistency()
    }

    pub(crate) fn remove_value_from_domain(
        &mut self,
        domain_id: DomainId,
        removed_value_from_domain: i32,
        reason: Option<ReasonRef>,
    ) -> Result<(), EmptyDomain> {
        // No need to do any changes if the value is not present anyway.
        if !self.domains[domain_id].contains(removed_value_from_domain) {
            return self.domains[domain_id].verify_consistency();
        }

        let predicate = Predicate::NotEqual {
            domain_id,
            not_equal_constant: removed_value_from_domain,
        };

        let old_lower_bound = self.get_lower_bound(domain_id);
        let old_upper_bound = self.get_upper_bound(domain_id);

        // important to record trail position _before_ pushing to the trail
        let trail_position = self.trail.len();

        self.trail.push(ConstraintProgrammingTrailEntry {
            predicate,
            old_lower_bound,
            old_upper_bound,
            reason,
        });

        let decision_level = self.get_decision_level();
        let domain = &mut self.domains[domain_id];

        domain.remove_value(
            removed_value_from_domain,
            decision_level,
            trail_position,
            &mut self.events,
        );

        domain.verify_consistency()
    }

    /// Apply the given [`Predicate`] to the integer domains.
    ///
    /// In case where the [`Predicate`] is already true, this does nothing. If instead
    /// applying the [`Predicate`] leads to an [`EmptyDomain`], the error variant is
    /// returned.
    pub(crate) fn post_predicate(
        &mut self,
        predicate: Predicate,
        reason: Option<ReasonRef>,
    ) -> Result<(), EmptyDomain> {
        match predicate {
            Predicate::LowerBound {
                domain_id,
                lower_bound,
            } => self.tighten_lower_bound(domain_id, lower_bound, reason),
            Predicate::UpperBound {
                domain_id,
                upper_bound,
            } => self.tighten_upper_bound(domain_id, upper_bound, reason),
            Predicate::NotEqual {
                domain_id,
                not_equal_constant,
            } => self.remove_value_from_domain(domain_id, not_equal_constant, reason),
            Predicate::Equal {
                domain_id,
                equality_constant,
            } => self.make_assignment(domain_id, equality_constant, reason),
        }
    }

    /// Determines whether the provided [`Predicate`] holds in the current state of the
    /// [`Assignments`]. In case the predicate is not assigned yet (neither true nor false),
    /// returns None.
    pub(crate) fn evaluate_predicate(&self, predicate: Predicate) -> Option<bool> {
        match predicate {
            Predicate::LowerBound {
                domain_id,
                lower_bound,
            } => {
                if self.get_lower_bound(domain_id) >= lower_bound {
                    Some(true)
                } else if self.get_upper_bound(domain_id) < lower_bound {
                    Some(false)
                } else {
                    None
                }
            }
            Predicate::UpperBound {
                domain_id,
                upper_bound,
            } => {
                if self.get_upper_bound(domain_id) <= upper_bound {
                    Some(true)
                } else if self.get_lower_bound(domain_id) > upper_bound {
                    Some(false)
                } else {
                    None
                }
            }
            Predicate::NotEqual {
                domain_id,
                not_equal_constant,
            } => {
                if !self.is_value_in_domain(domain_id, not_equal_constant) {
                    Some(true)
                } else if let Some(assigned_value) = self.get_assigned_value(domain_id) {
                    // Previous branch concluded the value is not in the domain, so if the variable
                    // is assigned, then it is assigned to the not equals value.
                    pumpkin_assert_simple!(assigned_value == not_equal_constant);
                    Some(false)
                } else {
                    None
                }
            }
            Predicate::Equal {
                domain_id,
                equality_constant,
            } => {
                if !self.is_value_in_domain(domain_id, equality_constant) {
                    Some(false)
                } else if let Some(assigned_value) = self.get_assigned_value(domain_id) {
                    pumpkin_assert_moderate!(assigned_value == equality_constant);
                    Some(true)
                } else {
                    None
                }
                // self
                //.get_assigned_value(domain_id)
                //.map(|assigned_value| assigned_value == equality_constant),
            }
        }
    }

    pub(crate) fn evaluate_predicate_at_trail_position(
        &self,
        predicate: Predicate,
        trail_position: usize,
    ) -> Option<bool> {
        match predicate {
            Predicate::LowerBound {
                domain_id,
                lower_bound,
            } => {
                if self.get_lower_bound_at_trail_position(domain_id, trail_position) >= lower_bound
                {
                    Some(true)
                } else if self.get_upper_bound_at_trail_position(domain_id, trail_position)
                    < lower_bound
                {
                    Some(false)
                } else {
                    None
                }
            }
            Predicate::UpperBound {
                domain_id,
                upper_bound,
            } => {
                if self.get_upper_bound_at_trail_position(domain_id, trail_position) <= upper_bound
                {
                    Some(true)
                } else if self.get_lower_bound_at_trail_position(domain_id, trail_position)
                    > upper_bound
                {
                    Some(false)
                } else {
                    None
                }
            }
            Predicate::NotEqual {
                domain_id,
                not_equal_constant,
            } => {
                if !self.is_value_in_domain_at_trail_position(
                    domain_id,
                    not_equal_constant,
                    trail_position,
                ) {
                    Some(true)
                } else if let Some(assigned_value) =
                    self.get_assigned_value_at_trail_position(domain_id, trail_position)
                {
                    // Previous branch concluded the value is not in the domain, so if the variable
                    // is assigned, then it is assigned to the not equals value.
                    pumpkin_assert_simple!(assigned_value == not_equal_constant);
                    Some(false)
                } else {
                    None
                }
            }
            Predicate::Equal {
                domain_id,
                equality_constant,
            } => {
                if !self.is_value_in_domain_at_trail_position(
                    domain_id,
                    equality_constant,
                    trail_position,
                ) {
                    Some(false)
                } else if let Some(assigned_value) =
                    self.get_assigned_value_at_trail_position(domain_id, trail_position)
                {
                    pumpkin_assert_moderate!(assigned_value == equality_constant);
                    Some(true)
                } else {
                    None
                }
                // self
                // .get_assigned_value_at_trail_position(domain_id, trail_position)
                // .map(|assigned_value| assigned_value == equality_constant),
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_predicate_satisfied(&self, predicate: Predicate) -> bool {
        self.evaluate_predicate(predicate)
            .is_some_and(|truth_value| truth_value)
    }

    #[allow(dead_code)]
    pub(crate) fn is_predicate_falsified(&self, predicate: Predicate) -> bool {
        self.evaluate_predicate(predicate)
            .is_some_and(|truth_value| !truth_value)
    }

    /// Synchronises the internal structures of [`Assignments`] based on the fact that
    /// backtracking to `new_decision_level` is taking place. This method returns the list of
    /// [`DomainId`]s and their values which were fixed (i.e. domain of size one) before
    /// backtracking and are unfixed (i.e. domain of two or more values) after synchronisation.
    pub(crate) fn synchronise(&mut self, new_decision_level: usize) -> Vec<(DomainId, i32)> {
        let mut unfixed_variables = Vec::new();
        self.trail.synchronise(new_decision_level).for_each(|entry| {
            pumpkin_assert_moderate!(
                !entry.predicate.is_equality_predicate(),
                "For now we do not expect equality predicates on the trail, since currently equality predicates are split into lower and upper bound predicates."
            );
            let domain_id = entry.predicate.get_domain();
            let fixed_before = self.domains[domain_id].lower_bound() == self.domains[domain_id].upper_bound();
                let value_before = self.domains[domain_id].lower_bound();
                self.domains[domain_id].undo_trail_entry(&entry);
                if fixed_before && self.domains[domain_id].lower_bound() != self.domains[domain_id].upper_bound() {
                    // Variable used to be fixed but is not after backtracking
                    unfixed_variables.push((domain_id, value_before));
                }
        });
        // Drain does not remove the events from the internal data structure. Elements are removed
        // lazily, as the iterator gets executed. For this reason we go through the entire iterator.
        let iter = self.events.drain();
        let _ = iter.count();
        // println!("ASSIGN AFTER SYNC PRESENT: {:?}", self.events.present);
        // println!("others: {:?}", self.events.events);
        unfixed_variables
    }

    /// todo: This is a temporary hack, not to be used in general.
    pub(crate) fn remove_last_trail_element(&mut self) {
        let entry = self.trail.pop().unwrap();
        let domain_id = entry.predicate.get_domain();
        self.domains[domain_id].undo_trail_entry(&entry);
    }
}

#[cfg(test)]
impl Assignments {
    pub(crate) fn get_reason_for_predicate_brute_force(&self, predicate: Predicate) -> ReasonRef {
        self.trail
            .iter()
            .find_map(|entry| {
                if entry.predicate == predicate {
                    entry.reason
                } else {
                    None
                }
            })
            .unwrap_or_else(|| panic!("could not find a reason for predicate {}", predicate))
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ConstraintProgrammingTrailEntry {
    pub(crate) predicate: Predicate,
    /// Explicitly store the bound before the predicate was applied so that it is easier later on
    ///  to update the bounds when backtracking.
    pub(crate) old_lower_bound: i32,
    pub(crate) old_upper_bound: i32,
    /// Stores the a reference to the reason in the `ReasonStore`, only makes sense if a
    /// propagation  took place, e.g., does _not_ make sense in the case of a decision or if
    /// the update was due  to synchronisation from the propositional trail.
    pub(crate) reason: Option<ReasonRef>,
}

#[derive(Clone, Copy, Debug)]
struct PairDecisionLevelTrailPosition {
    decision_level: usize,
    trail_position: usize,
}

#[derive(Clone, Debug)]
struct BoundUpdateInfo {
    bound: i32,
    decision_level: usize,
    trail_position: usize,
}

#[derive(Clone, Debug)]
struct HoleUpdateInfo {
    removed_value: i32,
    #[allow(dead_code)]
    decision_level: usize,
    #[allow(dead_code)]
    trail_position: usize,
    triggered_lower_bound_update: bool,
    triggered_upper_bound_update: bool,
}

/// This is the CP representation of a domain. It stores the bounds alongside holes in the domain.
/// When the domain is in an empty state, `lower_bound > upper_bound`.
/// The domain tracks all domain changes, so it is possible to query the domain at a given
/// cp trail position, i.e., the domain at some previous point in time.
/// This is needed to support lazy explanations.
#[derive(Clone, Debug)]
struct IntegerDomain {
    id: DomainId,
    /// The 'updates' fields chronologically records the changes to the domain.
    lower_bound_updates: Vec<BoundUpdateInfo>,
    upper_bound_updates: Vec<BoundUpdateInfo>,
    hole_updates: Vec<HoleUpdateInfo>,
    /// Auxiliary data structure to make it easy to check if a value is present or not.
    /// This is done to avoid going through 'hole_updates'.
    /// It maps a removed value with its decision level and trail position.
    /// In the future we could consider using direct hashing if the domain is small.
    holes: HashMap<i32, PairDecisionLevelTrailPosition>,
}

impl IntegerDomain {
    fn new(lower_bound: i32, upper_bound: i32, id: DomainId) -> IntegerDomain {
        pumpkin_assert_simple!(lower_bound <= upper_bound, "Cannot create an empty domain.");

        let lower_bound_updates = vec![BoundUpdateInfo {
            bound: lower_bound,
            decision_level: 0,
            trail_position: 0,
        }];

        let upper_bound_updates = vec![BoundUpdateInfo {
            bound: upper_bound,
            decision_level: 0,
            trail_position: 0,
        }];

        IntegerDomain {
            id,
            lower_bound_updates,
            upper_bound_updates,
            hole_updates: vec![],
            holes: Default::default(),
        }
    }

    fn lower_bound(&self) -> i32 {
        // the last entry contains the current lower bound
        self.lower_bound_updates
            .last()
            .expect("Cannot be empty.")
            .bound
    }

    fn initial_lower_bound(&self) -> i32 {
        // the first entry is never removed,
        // and contains the bound that was assigned upon creation
        self.lower_bound_updates[0].bound
    }

    fn lower_bound_at_trail_position(&self, trail_position: usize) -> i32 {
        // for now a simple inefficient linear scan
        // in the future this should be done with binary search
        // possibly caching old queries, and
        // maybe even first checking large/small trail position values
        // (in case those are commonly used)

        // find the update with largest trail position
        // that is smaller than or equal to the input trail position

        // Recall that by the nature of the updates,
        // the updates are stored in increasing order of trail position.
        self.lower_bound_updates
            .iter()
            .filter(|u| u.trail_position <= trail_position)
            .last()
            .expect("Cannot fail")
            .bound
    }

    fn upper_bound(&self) -> i32 {
        // the last entry contains the current upper bound
        self.upper_bound_updates
            .last()
            .expect("Cannot be empty.")
            .bound
    }

    fn initial_upper_bound(&self) -> i32 {
        // the first entry is never removed,
        // and contains the bound that was assigned upon creation
        self.upper_bound_updates[0].bound
    }

    fn upper_bound_at_trail_position(&self, trail_position: usize) -> i32 {
        // for now a simple inefficient linear scan
        // in the future this should be done with binary search
        // possibly caching old queries, and
        // maybe even first checking large/small trail position values
        // (in case those are commonly used)

        // find the update with largest trail position
        // that is smaller than or equal to the input trail position

        // Recall that by the nature of the updates,
        // the updates are stored in increasing order of trail position.
        self.upper_bound_updates
            .iter()
            .filter(|u| u.trail_position <= trail_position)
            .last()
            .expect("Cannot fail")
            .bound
    }

    #[allow(dead_code)]
    fn domain_iterator(&self) -> IntegerDomainIterator {
        // Ideally we use into_iter but I did not manage to get it to work,
        // because the iterator takes a lifelines
        // (the iterator takes a reference to the domain).
        // So this will do for now.
        IntegerDomainIterator::new(self)
    }

    fn contains(&self, value: i32) -> bool {
        self.lower_bound() <= value
            && value <= self.upper_bound()
            && !self.holes.contains_key(&value)
    }

    fn contains_at_trail_position(&self, value: i32, trail_position: usize) -> bool {
        // If the value is out of bounds,
        // then we can safety say that the value is not in the domain.
        if self.lower_bound_at_trail_position(trail_position) > value
            || self.upper_bound_at_trail_position(trail_position) < value
        {
            return false;
        }
        // Otherwise we need to check if there is a hole with that specific value.

        // In case the hole is made at the given trail position or earlier,
        // the value is not in the domain.
        if let Some(hole_info) = self.holes.get(&value) {
            if hole_info.trail_position <= trail_position {
                return false;
            }
        }
        // Since none of the previous checks triggered, the value is in the domain.
        true
    }

    fn remove_value(
        &mut self,
        removed_value: i32,
        decision_level: usize,
        trail_position: usize,
        events: &mut EventSink,
    ) {
        if removed_value < self.lower_bound()
            || removed_value > self.upper_bound()
            || self.holes.contains_key(&removed_value)
        {
            return;
        }

        events.event_occurred(IntDomainEvent::Removal, self.id);

        self.hole_updates.push(HoleUpdateInfo {
            removed_value,
            decision_level,
            trail_position,
            triggered_lower_bound_update: false,
            triggered_upper_bound_update: false,
        });
        // Note that it is important to remove the hole now,
        // because the later if statements may use the holes.
        let old_none_entry = self.holes.insert(
            removed_value,
            PairDecisionLevelTrailPosition {
                decision_level,
                trail_position,
            },
        );
        pumpkin_assert_moderate!(old_none_entry.is_none());

        // Check if removing a value triggers a lower bound update.
        if self.lower_bound() == removed_value {
            self.set_lower_bound(removed_value + 1, decision_level, trail_position, events);
            self.hole_updates
                .last_mut()
                .expect("we just pushed a value, so must be present")
                .triggered_lower_bound_update = true;
        }
        // Check if removing the value triggers an upper bound update.
        if self.upper_bound() == removed_value {
            self.set_upper_bound(removed_value - 1, decision_level, trail_position, events);
            self.hole_updates
                .last_mut()
                .expect("we just pushed a value, so must be present")
                .triggered_upper_bound_update = true;
        }

        if self.lower_bound() == self.upper_bound() {
            events.event_occurred(IntDomainEvent::Assign, self.id);
        }
    }

    fn debug_is_valid_upper_bound_domain_update(
        &self,
        decision_level: usize,
        trail_position: usize,
    ) -> bool {
        trail_position == 0
            || self.upper_bound_updates.last().unwrap().decision_level <= decision_level
                && self.upper_bound_updates.last().unwrap().trail_position < trail_position
    }

    fn set_upper_bound(
        &mut self,
        new_upper_bound: i32,
        decision_level: usize,
        trail_position: usize,
        events: &mut EventSink,
    ) {
        pumpkin_assert_moderate!(
            self.debug_is_valid_upper_bound_domain_update(decision_level, trail_position)
        );

        if new_upper_bound >= self.upper_bound() {
            return;
        }

        events.event_occurred(IntDomainEvent::UpperBound, self.id);

        self.upper_bound_updates.push(BoundUpdateInfo {
            bound: new_upper_bound,
            decision_level,
            trail_position,
        });
        self.update_upper_bound_with_respect_to_holes();

        if self.lower_bound() == self.upper_bound() {
            events.event_occurred(IntDomainEvent::Assign, self.id);
        }
    }

    fn update_upper_bound_with_respect_to_holes(&mut self) {
        while self.holes.contains_key(&self.upper_bound())
            && self.lower_bound() <= self.upper_bound()
        {
            self.upper_bound_updates.last_mut().unwrap().bound -= 1;
        }
    }

    fn debug_is_valid_lower_bound_domain_update(
        &self,
        decision_level: usize,
        trail_position: usize,
    ) -> bool {
        trail_position == 0
            || self.lower_bound_updates.last().unwrap().decision_level <= decision_level
                && self.lower_bound_updates.last().unwrap().trail_position < trail_position
    }

    fn set_lower_bound(
        &mut self,
        new_lower_bound: i32,
        decision_level: usize,
        trail_position: usize,
        events: &mut EventSink,
    ) {
        pumpkin_assert_moderate!(
            self.debug_is_valid_lower_bound_domain_update(decision_level, trail_position)
        );

        if new_lower_bound <= self.lower_bound() {
            return;
        }

        events.event_occurred(IntDomainEvent::LowerBound, self.id);

        self.lower_bound_updates.push(BoundUpdateInfo {
            bound: new_lower_bound,
            decision_level,
            trail_position,
        });
        self.update_lower_bound_with_respect_to_holes();

        if self.lower_bound() == self.upper_bound() {
            events.event_occurred(IntDomainEvent::Assign, self.id);
        }
    }

    fn update_lower_bound_with_respect_to_holes(&mut self) {
        while self.holes.contains_key(&self.lower_bound())
            && self.lower_bound() <= self.upper_bound()
        {
            self.lower_bound_updates.last_mut().unwrap().bound += 1;
        }
    }

    fn debug_bounds_check(&self) -> bool {
        // If the domain is empty, the lower bound will be greater than the upper bound.
        if self.lower_bound() > self.upper_bound() {
            true
        } else {
            self.lower_bound() >= self.initial_lower_bound()
                && self.upper_bound() <= self.initial_upper_bound()
                && !self.holes.contains_key(&self.lower_bound())
                && !self.holes.contains_key(&self.upper_bound())
        }
    }

    fn verify_consistency(&self) -> Result<(), EmptyDomain> {
        if self.lower_bound() > self.upper_bound() {
            Err(EmptyDomain)
        } else {
            Ok(())
        }
    }

    fn undo_trail_entry(&mut self, entry: &ConstraintProgrammingTrailEntry) {
        match entry.predicate {
            Predicate::LowerBound {
                domain_id,
                lower_bound: _,
            } => {
                pumpkin_assert_moderate!(domain_id == self.id);

                let _ = self.lower_bound_updates.pop();
                pumpkin_assert_moderate!(!self.lower_bound_updates.is_empty());
            }
            Predicate::UpperBound {
                domain_id,
                upper_bound: _,
            } => {
                pumpkin_assert_moderate!(domain_id == self.id);

                let _ = self.upper_bound_updates.pop();
                pumpkin_assert_moderate!(!self.upper_bound_updates.is_empty());
            }
            Predicate::NotEqual {
                domain_id,
                not_equal_constant,
            } => {
                pumpkin_assert_moderate!(domain_id == self.id);

                let hole_update = self
                    .hole_updates
                    .pop()
                    .expect("Must have record of domain removal.");
                pumpkin_assert_moderate!(hole_update.removed_value == not_equal_constant);

                let _ = self
                    .holes
                    .remove(&not_equal_constant)
                    .expect("Must be present.");

                if hole_update.triggered_lower_bound_update {
                    let _ = self.lower_bound_updates.pop();
                    pumpkin_assert_moderate!(!self.lower_bound_updates.is_empty());
                }

                if hole_update.triggered_upper_bound_update {
                    let _ = self.upper_bound_updates.pop();
                    pumpkin_assert_moderate!(!self.upper_bound_updates.is_empty());
                }
            }
            Predicate::Equal {
                domain_id: _,
                equality_constant: _,
            } => {
                // I think we never push equality predicates to the trail
                // in the current version. Equality gets substituted
                // by a lower and upper bound predicate.
                unreachable!()
            }
        };

        // these asserts will be removed, for now it is a sanity check
        // later we may remove the old bound from the trail entry since it is not needed
        pumpkin_assert_simple!(self.lower_bound() == entry.old_lower_bound);
        pumpkin_assert_simple!(self.upper_bound() == entry.old_upper_bound);

        pumpkin_assert_moderate!(self.debug_bounds_check());
    }

    fn get_update_info(&self, predicate: &Predicate) -> Option<PairDecisionLevelTrailPosition> {
        // Perhaps the recursion could be done in a cleaner way,
        // e.g., separate functions dependibng on the type of predicate.
        // For the initial version, the current version is okay.
        match predicate {
            Predicate::LowerBound {
                domain_id: _,
                lower_bound,
            } => {
                // Recall that by the nature of the updates,
                // the updates are stored in increasing order of the lower bound.

                // for now a simple inefficient linear scan
                // in the future this should be done with binary search

                // find the update with smallest lower bound
                // that is greater than or equal to the input lower bound
                self.lower_bound_updates
                    .iter()
                    .find(|u| u.bound >= *lower_bound)
                    .map(|u| PairDecisionLevelTrailPosition {
                        decision_level: u.decision_level,
                        trail_position: u.trail_position,
                    })
            }
            Predicate::UpperBound {
                domain_id: _,
                upper_bound,
            } => {
                // Recall that by the nature of the updates,
                // the updates are stored in decreasing order of the upper bound.

                // for now a simple inefficient linear scan
                // in the future this should be done with binary search

                // find the update with greatest upper bound
                // that is smaller than or equal to the input upper bound
                self.upper_bound_updates
                    .iter()
                    .find(|u| u.bound <= *upper_bound)
                    .map(|u| PairDecisionLevelTrailPosition {
                        decision_level: u.decision_level,
                        trail_position: u.trail_position,
                    })
            }
            Predicate::NotEqual {
                domain_id,
                not_equal_constant,
            } => {
                // Check the explictly stored holes.
                // If the value has been removed explicitly,
                // then the stored time is the first time the value was removed.
                if let Some(hole_info) = self.holes.get(not_equal_constant) {
                    Some(*hole_info)
                } else {
                    // Otherwise, check the case when the lower/upper bound surpassed the value.
                    // If this never happened, then report that the predicate is not true.

                    // Note that it cannot be that both the lower bound and upper bound surpassed
                    // the not equals constant, i.e., at most one of the two may happen.
                    // So we can stop as soon as we find one of the two.

                    // Check the lower bound first.
                    if let Some(trail_position) = self.get_update_info(&Predicate::LowerBound {
                        domain_id: *domain_id,
                        lower_bound: not_equal_constant + 1,
                    }) {
                        // The lower bound removed the value from the domain,
                        // report the trail position of the lower bound.
                        Some(trail_position)
                    } else {
                        // The lower bound did not surpass the value,
                        // now check the upper bound.
                        self.get_update_info(&Predicate::UpperBound {
                            domain_id: *domain_id,
                            upper_bound: not_equal_constant - 1,
                        })
                    }
                }
            }
            Predicate::Equal {
                domain_id,
                equality_constant,
            } => {
                // For equality to hold, both the lower and upper bound predicates must hold.
                // Check lower bound first.
                if let Some(lb_trail_position) = self.get_update_info(&Predicate::LowerBound {
                    domain_id: *domain_id,
                    lower_bound: *equality_constant,
                }) {
                    // The lower bound found,
                    // now the check depends on the upper bound.

                    // If both the lower and upper bounds are present,
                    // report the trail position of the bound that was set last.
                    // Otherwise, return that the predicate is not on the trail.
                    self.get_update_info(&Predicate::UpperBound {
                        domain_id: *domain_id,
                        upper_bound: *equality_constant,
                    })
                    .map(|ub_trail_position| {
                        if lb_trail_position.trail_position > ub_trail_position.trail_position {
                            lb_trail_position
                        } else {
                            ub_trail_position
                        }
                    })
                }
                // If the lower bound is never reached,
                // then surely the equality predicate cannot be true.
                else {
                    None
                }
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct IntegerDomainIterator<'a> {
    domain: &'a IntegerDomain,
    current_value: i32,
}

impl IntegerDomainIterator<'_> {
    #[allow(dead_code)]
    fn new(domain: &IntegerDomain) -> IntegerDomainIterator {
        IntegerDomainIterator {
            domain,
            current_value: domain.lower_bound(),
        }
    }
}

impl Iterator for IntegerDomainIterator<'_> {
    type Item = i32;
    fn next(&mut self) -> Option<i32> {
        // We would not expect to iterate through inconsistent domains,
        // although we support trying to do so. Not sure if this is good a idea?
        if self.domain.verify_consistency().is_err() {
            return None;
        }

        // Note that the current value is never a hole. This is guaranteed by 1) having
        // a consistent domain, 2) the iterator starts with the lower bound,
        // and 3) the while loop after this if statement updates the current value
        // to a non-hole value (if there are any left within the bounds).
        let result = if self.current_value <= self.domain.upper_bound() {
            Some(self.current_value)
        } else {
            None
        };

        self.current_value += 1;
        // If the current value is within the bounds, but is not in the domain,
        // linearly look for the next non-hole value.
        while self.current_value <= self.domain.upper_bound()
            && !self.domain.contains(self.current_value)
        {
            self.current_value += 1;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_bound_change_lower_bound_event() {
        let mut assignment = Assignments::default();
        let d1 = assignment.grow(1, 5);

        assignment
            .tighten_lower_bound(d1, 2, None)
            .expect("non-empty domain");

        let events = assignment.drain_domain_events().collect::<Vec<_>>();
        assert_eq!(events.len(), 1);

        assert_contains_events(&events, d1, [IntDomainEvent::LowerBound]);
    }

    #[test]
    fn upper_bound_change_triggers_upper_bound_event() {
        let mut assignment = Assignments::default();
        let d1 = assignment.grow(1, 5);

        assignment
            .tighten_upper_bound(d1, 2, None)
            .expect("non-empty domain");

        let events = assignment.drain_domain_events().collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert_contains_events(&events, d1, [IntDomainEvent::UpperBound]);
    }

    #[test]
    fn bounds_change_can_also_trigger_assign_event() {
        let mut assignment = Assignments::default();

        let d1 = assignment.grow(1, 5);
        let d2 = assignment.grow(1, 5);

        assignment
            .tighten_lower_bound(d1, 5, None)
            .expect("non-empty domain");
        assignment
            .tighten_upper_bound(d2, 1, None)
            .expect("non-empty domain");

        let events = assignment.drain_domain_events().collect::<Vec<_>>();
        assert_eq!(events.len(), 4);

        assert_contains_events(
            &events,
            d1,
            [IntDomainEvent::LowerBound, IntDomainEvent::Assign],
        );
        assert_contains_events(
            &events,
            d2,
            [IntDomainEvent::UpperBound, IntDomainEvent::Assign],
        );
    }

    #[test]
    fn making_assignment_triggers_appropriate_events() {
        let mut assignment = Assignments::default();

        let d1 = assignment.grow(1, 5);
        let d2 = assignment.grow(1, 5);
        let d3 = assignment.grow(1, 5);

        assignment
            .make_assignment(d1, 1, None)
            .expect("non-empty domain");
        assignment
            .make_assignment(d2, 5, None)
            .expect("non-empty domain");
        assignment
            .make_assignment(d3, 3, None)
            .expect("non-empty domain");

        let events = assignment.drain_domain_events().collect::<Vec<_>>();
        assert_eq!(events.len(), 7);

        assert_contains_events(
            &events,
            d1,
            [IntDomainEvent::Assign, IntDomainEvent::UpperBound],
        );
        assert_contains_events(
            &events,
            d2,
            [IntDomainEvent::Assign, IntDomainEvent::LowerBound],
        );
        assert_contains_events(
            &events,
            d3,
            [
                IntDomainEvent::Assign,
                IntDomainEvent::LowerBound,
                IntDomainEvent::UpperBound,
            ],
        );
    }

    #[test]
    fn removal_triggers_removal_event() {
        let mut assignment = Assignments::default();
        let d1 = assignment.grow(1, 5);

        assignment
            .remove_value_from_domain(d1, 2, None)
            .expect("non-empty domain");

        let events = assignment.drain_domain_events().collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert!(events.contains(&(IntDomainEvent::Removal, d1)));
    }

    #[test]
    fn values_can_be_removed_from_domains() {
        let mut events = EventSink::default();
        events.grow();

        let mut domain = IntegerDomain::new(1, 5, DomainId::new(0));
        domain.remove_value(1, 1, 2, &mut events);

        assert!(!domain.contains(2));
    }

    #[test]
    fn removing_the_lower_bound_updates_that_lower_bound() {
        let mut events = EventSink::default();
        events.grow();

        let mut domain = IntegerDomain::new(1, 5, DomainId::new(0));
        domain.remove_value(1, 1, 1, &mut events);
        domain.remove_value(1, 1, 2, &mut events);

        assert_eq!(3, domain.lower_bound());
    }

    #[test]
    fn removing_the_upper_bound_updates_the_upper_bound() {
        let mut events = EventSink::default();
        events.grow();

        let mut domain = IntegerDomain::new(1, 5, DomainId::new(0));
        domain.remove_value(4, 0, 1, &mut events);
        domain.remove_value(5, 0, 2, &mut events);

        assert_eq!(3, domain.upper_bound());
    }

    #[test]
    fn an_empty_domain_accepts_removal_operations() {
        let mut events = EventSink::default();
        events.grow();

        let mut domain = IntegerDomain::new(1, 5, DomainId::new(0));
        domain.remove_value(4, 0, 1, &mut events);
        domain.remove_value(1, 0, 2, &mut events);
        domain.remove_value(1, 0, 3, &mut events);
    }

    #[test]
    fn setting_lower_bound_rounds_up_to_nearest_value_in_domain() {
        let mut events = EventSink::default();
        events.grow();

        let mut domain = IntegerDomain::new(1, 5, DomainId::new(0));
        domain.remove_value(1, 1, 1, &mut events);
        domain.set_lower_bound(2, 1, 2, &mut events);

        assert_eq!(3, domain.lower_bound());
    }

    #[test]
    fn setting_upper_bound_rounds_down_to_nearest_value_in_domain() {
        let mut events = EventSink::default();
        events.grow();

        let mut domain = IntegerDomain::new(1, 5, DomainId::new(0));
        domain.remove_value(4, 0, 1, &mut events);
        domain.set_upper_bound(4, 0, 2, &mut events);

        assert_eq!(3, domain.upper_bound());
    }

    #[test]
    fn undo_removal_at_bounds_indexes_into_values_domain_correctly() {
        let mut assignment = Assignments::default();
        let d1 = assignment.grow(1, 5);

        assignment.increase_decision_level();

        assignment
            .remove_value_from_domain(d1, 5, None)
            .expect("non-empty domain");

        let _ = assignment.synchronise(0);

        assert_eq!(5, assignment.get_upper_bound(d1));
    }

    fn assert_contains_events(
        slice: &[(IntDomainEvent, DomainId)],
        domain: DomainId,
        required_events: impl AsRef<[IntDomainEvent]>,
    ) {
        for event in required_events.as_ref() {
            assert!(slice.contains(&(*event, domain)));
        }
    }

    fn get_domain1() -> (DomainId, IntegerDomain, EventSink) {
        let mut events = EventSink::default();
        events.grow();

        let domain_id = DomainId::new(0);
        let mut domain = IntegerDomain::new(0, 100, domain_id);
        domain.set_lower_bound(1, 0, 1, &mut events);
        domain.set_lower_bound(5, 1, 2, &mut events);
        domain.set_lower_bound(10, 2, 10, &mut events);
        domain.set_lower_bound(20, 5, 50, &mut events);
        domain.set_lower_bound(50, 10, 70, &mut events);

        (domain_id, domain, events)
    }

    #[test]
    fn lower_bound_trail_position_inbetween_value() {
        let (domain_id, domain, _) = get_domain1();

        assert_eq!(
            domain
                .get_update_info(&Predicate::LowerBound {
                    domain_id,
                    lower_bound: 12,
                })
                .unwrap()
                .trail_position,
            50
        );
    }

    #[test]
    fn lower_bound_trail_position_last_bound() {
        let (domain_id, domain, _) = get_domain1();

        assert_eq!(
            domain
                .get_update_info(&Predicate::LowerBound {
                    domain_id,
                    lower_bound: 50,
                })
                .unwrap()
                .trail_position,
            70
        );
    }

    #[test]
    fn lower_bound_trail_position_beyond_value() {
        let (domain_id, domain, _) = get_domain1();

        assert!(domain
            .get_update_info(&Predicate::LowerBound {
                domain_id,
                lower_bound: 101,
            })
            .is_none());
    }

    #[test]
    fn lower_bound_trail_position_trivial() {
        let (domain_id, domain, _) = get_domain1();

        assert_eq!(
            domain
                .get_update_info(&Predicate::LowerBound {
                    domain_id,
                    lower_bound: -10,
                })
                .unwrap()
                .trail_position,
            0
        );
    }

    #[test]
    fn lower_bound_trail_position_with_removals() {
        let (domain_id, mut domain, mut events) = get_domain1();
        domain.remove_value(50, 11, 75, &mut events);
        domain.remove_value(51, 11, 77, &mut events);
        domain.remove_value(52, 11, 80, &mut events);

        assert_eq!(
            domain
                .get_update_info(&Predicate::LowerBound {
                    domain_id,
                    lower_bound: 52,
                })
                .unwrap()
                .trail_position,
            77
        );
    }

    #[test]
    fn removal_trail_position() {
        let (domain_id, mut domain, mut events) = get_domain1();
        domain.remove_value(50, 11, 75, &mut events);
        domain.remove_value(51, 11, 77, &mut events);
        domain.remove_value(52, 11, 80, &mut events);

        assert_eq!(
            domain
                .get_update_info(&Predicate::NotEqual {
                    domain_id,
                    not_equal_constant: 50,
                })
                .unwrap()
                .trail_position,
            75
        );
    }

    #[test]
    fn removal_trail_position_after_lower_bound() {
        let (domain_id, mut domain, mut events) = get_domain1();
        domain.remove_value(50, 11, 75, &mut events);
        domain.remove_value(51, 11, 77, &mut events);
        domain.remove_value(52, 11, 80, &mut events);
        domain.set_lower_bound(60, 11, 150, &mut events);

        assert_eq!(
            domain
                .get_update_info(&Predicate::NotEqual {
                    domain_id,
                    not_equal_constant: 55,
                })
                .unwrap()
                .trail_position,
            150
        );
    }

    #[test]
    fn lower_bound_change_backtrack() {
        let mut assignment = Assignments::default();
        let domain_id1 = assignment.grow(0, 100);
        let domain_id2 = assignment.grow(0, 50);

        // decision level 1
        assignment.increase_decision_level();
        assignment
            .post_predicate(
                Predicate::LowerBound {
                    domain_id: domain_id1,
                    lower_bound: 2,
                },
                None,
            )
            .expect("");
        assignment
            .post_predicate(
                Predicate::LowerBound {
                    domain_id: domain_id2,
                    lower_bound: 25,
                },
                None,
            )
            .expect("");

        // decision level 2
        assignment.increase_decision_level();
        assignment
            .post_predicate(
                Predicate::LowerBound {
                    domain_id: domain_id1,
                    lower_bound: 5,
                },
                None,
            )
            .expect("");

        // decision level 3
        assignment.increase_decision_level();
        assignment
            .post_predicate(
                Predicate::LowerBound {
                    domain_id: domain_id1,
                    lower_bound: 7,
                },
                None,
            )
            .expect("");

        assert_eq!(assignment.get_lower_bound(domain_id1), 7);

        let _ = assignment.synchronise(1);

        assert_eq!(assignment.get_lower_bound(domain_id1), 2);
    }

    #[test]
    fn lower_bound_inbetween_updates() {
        let (_, domain, _) = get_domain1();
        assert_eq!(domain.lower_bound_at_trail_position(25), 10);
    }

    #[test]
    fn lower_bound_beyond_trail_position() {
        let (_, domain, _) = get_domain1();
        assert_eq!(domain.lower_bound_at_trail_position(1000), 50);
    }

    #[test]
    fn lower_bound_at_update() {
        let (_, domain, _) = get_domain1();
        assert_eq!(domain.lower_bound_at_trail_position(50), 20);
    }

    #[test]
    fn lower_bound_at_trail_position_after_removals() {
        let (_, mut domain, mut events) = get_domain1();
        domain.remove_value(50, 11, 75, &mut events);
        domain.remove_value(51, 11, 77, &mut events);
        domain.remove_value(52, 11, 80, &mut events);

        assert_eq!(domain.lower_bound_at_trail_position(77), 52);
    }

    #[test]
    fn lower_bound_at_trail_position_after_removals_and_bound_update() {
        let (_, mut domain, mut events) = get_domain1();
        domain.remove_value(50, 11, 75, &mut events);
        domain.remove_value(51, 11, 77, &mut events);
        domain.remove_value(52, 11, 80, &mut events);
        domain.set_lower_bound(60, 11, 150, &mut events);

        assert_eq!(domain.lower_bound_at_trail_position(100), 53);
    }

    #[test]
    fn inconsistent_bound_updates() {
        let mut events = EventSink::default();
        events.grow();

        let domain_id = DomainId::new(0);
        let mut domain = IntegerDomain::new(0, 2, domain_id);
        domain.set_lower_bound(2, 1, 1, &mut events);
        domain.set_upper_bound(1, 1, 2, &mut events);
        assert!(domain.verify_consistency().is_err());
    }

    #[test]
    fn inconsistent_domain_removals() {
        let mut events = EventSink::default();
        events.grow();

        let domain_id = DomainId::new(0);
        let mut domain = IntegerDomain::new(0, 2, domain_id);
        domain.remove_value(1, 1, 1, &mut events);
        domain.remove_value(2, 1, 2, &mut events);
        domain.remove_value(0, 1, 3, &mut events);
        assert!(domain.verify_consistency().is_err());
    }

    #[test]
    fn domain_iterator_simple() {
        let domain_id = DomainId::new(0);
        let domain = IntegerDomain::new(0, 5, domain_id);
        let mut iter = domain.domain_iterator();
        assert_eq!(iter.next(), Some(0));
        assert_eq!(iter.next(), Some(1));
        assert_eq!(iter.next(), Some(2));
        assert_eq!(iter.next(), Some(3));
        assert_eq!(iter.next(), Some(4));
        assert_eq!(iter.next(), Some(5));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn domain_iterator_skip_holes() {
        let mut events = EventSink::default();
        events.grow();
        let domain_id = DomainId::new(0);
        let mut domain = IntegerDomain::new(0, 5, domain_id);
        domain.remove_value(1, 0, 5, &mut events);
        domain.remove_value(4, 0, 10, &mut events);

        let mut iter = domain.domain_iterator();
        assert_eq!(iter.next(), Some(0));
        assert_eq!(iter.next(), Some(1));
        assert_eq!(iter.next(), Some(3));
        assert_eq!(iter.next(), Some(5));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn domain_iterator_removed_bounds() {
        let mut events = EventSink::default();
        events.grow();
        let domain_id = DomainId::new(0);
        let mut domain = IntegerDomain::new(0, 5, domain_id);
        domain.remove_value(0, 0, 1, &mut events);
        domain.remove_value(5, 0, 10, &mut events);

        let mut iter = domain.domain_iterator();
        assert_eq!(iter.next(), Some(1));
        assert_eq!(iter.next(), Some(2));
        assert_eq!(iter.next(), Some(3));
        assert_eq!(iter.next(), Some(4));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn domain_iterator_removed_values_present_beyond_bounds() {
        let mut events = EventSink::default();
        events.grow();
        let domain_id = DomainId::new(0);
        let mut domain = IntegerDomain::new(0, 10, domain_id);
        domain.remove_value(7, 0, 1, &mut events);
        domain.remove_value(9, 0, 5, &mut events);
        domain.remove_value(7, 0, 10, &mut events);
        domain.set_upper_bound(6, 1, 10, &mut events);

        let mut iter = domain.domain_iterator();
        assert_eq!(iter.next(), Some(0));
        assert_eq!(iter.next(), Some(1));
        assert_eq!(iter.next(), Some(3));
        assert_eq!(iter.next(), Some(4));
        assert_eq!(iter.next(), Some(5));
        assert_eq!(iter.next(), Some(6));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn various_tests_evaluate_predicate() {
        let mut assignments = Assignments::default();
        // Create the domain {0, 1, 3, 4, 5, 6}
        let domain_id = assignments.grow(0, 10);
        let _ = assignments.remove_value_from_domain(domain_id, 7, None);
        let _ = assignments.remove_value_from_domain(domain_id, 9, None);
        let _ = assignments.remove_value_from_domain(domain_id, 2, None);
        let _ = assignments.tighten_upper_bound(domain_id, 6, None);

        let lb_predicate = |lower_bound: i32| -> Predicate {
            Predicate::LowerBound {
                domain_id,
                lower_bound,
            }
        };

        let ub_predicate = |upper_bound: i32| -> Predicate {
            Predicate::UpperBound {
                domain_id,
                upper_bound,
            }
        };

        let eq_predicate = |equality_constant: i32| -> Predicate {
            Predicate::Equal {
                domain_id,
                equality_constant,
            }
        };

        let neq_predicate = |not_equal_constant: i32| -> Predicate {
            Predicate::NotEqual {
                domain_id,
                not_equal_constant,
            }
        };

        assert!(assignments
            .evaluate_predicate(lb_predicate(0))
            .is_some_and(|x| x));
        assert!(assignments.evaluate_predicate(lb_predicate(1)).is_none());
        assert!(assignments.evaluate_predicate(lb_predicate(2)).is_none());
        assert!(assignments.evaluate_predicate(lb_predicate(3)).is_none());
        assert!(assignments.evaluate_predicate(lb_predicate(4)).is_none());
        assert!(assignments.evaluate_predicate(lb_predicate(5)).is_none());
        assert!(assignments.evaluate_predicate(lb_predicate(6)).is_none());
        assert!(assignments
            .evaluate_predicate(lb_predicate(7))
            .is_some_and(|x| !x));
        assert!(assignments
            .evaluate_predicate(lb_predicate(8))
            .is_some_and(|x| !x));
        assert!(assignments
            .evaluate_predicate(lb_predicate(9))
            .is_some_and(|x| !x));
        assert!(assignments
            .evaluate_predicate(lb_predicate(10))
            .is_some_and(|x| !x));

        assert!(assignments.evaluate_predicate(ub_predicate(0)).is_none());
        assert!(assignments.evaluate_predicate(ub_predicate(1)).is_none());
        assert!(assignments.evaluate_predicate(ub_predicate(2)).is_none());
        assert!(assignments.evaluate_predicate(ub_predicate(3)).is_none());
        assert!(assignments.evaluate_predicate(ub_predicate(4)).is_none());
        assert!(assignments.evaluate_predicate(ub_predicate(5)).is_none());
        assert!(assignments
            .evaluate_predicate(ub_predicate(6))
            .is_some_and(|x| x));
        assert!(assignments
            .evaluate_predicate(ub_predicate(7))
            .is_some_and(|x| x));
        assert!(assignments
            .evaluate_predicate(ub_predicate(8))
            .is_some_and(|x| x));
        assert!(assignments
            .evaluate_predicate(ub_predicate(9))
            .is_some_and(|x| x));
        assert!(assignments
            .evaluate_predicate(ub_predicate(10))
            .is_some_and(|x| x));

        assert!(assignments.evaluate_predicate(neq_predicate(0)).is_none());
        assert!(assignments.evaluate_predicate(neq_predicate(1)).is_none());
        assert!(assignments
            .evaluate_predicate(neq_predicate(2))
            .is_some_and(|x| x));
        assert!(assignments.evaluate_predicate(neq_predicate(3)).is_none());
        assert!(assignments.evaluate_predicate(neq_predicate(4)).is_none());
        assert!(assignments.evaluate_predicate(neq_predicate(5)).is_none());
        assert!(assignments.evaluate_predicate(neq_predicate(6)).is_none());
        assert!(assignments
            .evaluate_predicate(neq_predicate(7))
            .is_some_and(|x| x));
        assert!(assignments
            .evaluate_predicate(neq_predicate(8))
            .is_some_and(|x| x));
        assert!(assignments
            .evaluate_predicate(neq_predicate(9))
            .is_some_and(|x| x));
        assert!(assignments
            .evaluate_predicate(neq_predicate(10))
            .is_some_and(|x| x));

        assert!(assignments.evaluate_predicate(eq_predicate(0)).is_none());
        assert!(assignments.evaluate_predicate(eq_predicate(1)).is_none());
        assert!(assignments
            .evaluate_predicate(eq_predicate(2))
            .is_some_and(|x| !x));
        assert!(assignments.evaluate_predicate(eq_predicate(3)).is_none());
        assert!(assignments.evaluate_predicate(eq_predicate(4)).is_none());
        assert!(assignments.evaluate_predicate(eq_predicate(5)).is_none());
        assert!(assignments.evaluate_predicate(eq_predicate(6)).is_none());
        assert!(assignments
            .evaluate_predicate(eq_predicate(7))
            .is_some_and(|x| !x));
        assert!(assignments
            .evaluate_predicate(eq_predicate(8))
            .is_some_and(|x| !x));
        assert!(assignments
            .evaluate_predicate(eq_predicate(9))
            .is_some_and(|x| !x));
        assert!(assignments
            .evaluate_predicate(eq_predicate(10))
            .is_some_and(|x| !x));

        let _ = assignments.tighten_lower_bound(domain_id, 6, None);

        assert!(assignments
            .evaluate_predicate(neq_predicate(6))
            .is_some_and(|x| !x));
        assert!(assignments
            .evaluate_predicate(eq_predicate(6))
            .is_some_and(|x| x));
        assert!(assignments
            .evaluate_predicate(lb_predicate(6))
            .is_some_and(|x| x));
        assert!(assignments
            .evaluate_predicate(ub_predicate(6))
            .is_some_and(|x| x));
    }
}
