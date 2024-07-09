use crate::branching::SelectionContext;
use crate::branching::ValueSelector;
use crate::engine::predicates::integer_predicate::IntegerPredicate;
use crate::engine::variables::IntegerVariable;
use crate::predicate;
use crate::pumpkin_assert_advanced;

/// A [`ValueSelector`] which splits the domain in half (based on the lower-bound and upper-bound,
/// disregarding holes) and removes the lower-half from the domain.
///
/// Note that this strategy will not necessarily result in an equal split if there are holes in the
/// domain.
#[derive(Debug, Copy, Clone)]
pub struct ReverseInDomainSplit;

impl<Var: IntegerVariable + Copy> ValueSelector<Var> for ReverseInDomainSplit {
    fn select_value(
        &mut self,
        context: &mut SelectionContext,
        decision_variable: Var,
    ) -> IntegerPredicate {
        // Note that the domain of the variable should always have at least 2 values in it
        // (otherwise it should have been reported as fixed and not selected)
        let bound = context.lower_bound(decision_variable)
            + (context.get_size_of_domain(decision_variable) as f64 / 2.0).ceil() as i32;
        pumpkin_assert_advanced!(
            bound > context.lower_bound(decision_variable)
                && bound <= context.upper_bound(decision_variable),
            "It should hold that {} < {bound} <= {}",
            context.lower_bound(decision_variable),
            context.upper_bound(decision_variable)
        );
        predicate!(decision_variable >= bound)
    }
}

#[cfg(test)]
mod tests {
    use crate::basic_types::tests::TestRandom;
    use crate::branching::ReverseInDomainSplit;
    use crate::branching::SelectionContext;
    use crate::branching::ValueSelector;
    use crate::predicate;

    #[test]
    fn test_returns_correct_literal() {
        let assignments_integer = SelectionContext::create_for_testing(vec![(0, 10)]);
        let mut test_rng = TestRandom::default();
        let mut context = SelectionContext::new(&assignments_integer, &mut test_rng);
        let domain_ids = context.get_domains().collect::<Vec<_>>();

        let mut selector = ReverseInDomainSplit;

        let selected_predicate = selector.select_value(&mut context, domain_ids[0]);

        assert_eq!(selected_predicate, predicate!(domain_ids[0] >= 5))
    }

    #[test]
    fn test_domain_of_size_two() {
        let assignments_integer = SelectionContext::create_for_testing(vec![(1, 2)]);
        let mut test_rng = TestRandom::default();
        let mut context = SelectionContext::new(&assignments_integer, &mut test_rng);
        let domain_ids = context.get_domains().collect::<Vec<_>>();

        let mut selector = ReverseInDomainSplit;

        let selected_predicate = selector.select_value(&mut context, domain_ids[0]);

        assert_eq!(selected_predicate, predicate!(domain_ids[0] >= 2))
    }
}
