use std::marker::PhantomData;

use crate::basic_types::SolutionReference;
use crate::branching::Brancher;
use crate::branching::InDomainMin;
use crate::branching::InputOrder;
use crate::branching::SelectionContext;
use crate::branching::ValueSelector;
use crate::branching::VariableSelector;
use crate::engine::predicates::integer_predicate::IntegerPredicate;
use crate::engine::variables::DomainId;
use crate::engine::ConstraintSatisfactionSolver;

/// An implementation of a [`Brancher`] which simply uses a single
/// [`VariableSelector`] and a single [`ValueSelector`] independently of one another.
#[derive(Debug)]
pub struct IndependentVariableValueBrancher<Var, VariableSelect, ValueSelect>
where
    VariableSelect: VariableSelector<Var>,
    ValueSelect: ValueSelector<Var>,
{
    /// The [`VariableSelector`] of the [`Brancher`], determines which (unfixed) variable to branch
    /// next on.
    variable_selector: VariableSelect,
    /// The [`ValueSelector`] of the [`Brancher`] determines which value in the domain to branch
    /// next on given a variable.
    value_selector: ValueSelect,
    /// [`PhantomData`] to ensure that the variable type is bound to the
    /// [`IndependentVariableValueBrancher`]
    variable_type: PhantomData<Var>,
}

impl<Var, VariableSelect, ValueSelect>
    IndependentVariableValueBrancher<Var, VariableSelect, ValueSelect>
where
    VariableSelect: VariableSelector<Var>,
    ValueSelect: ValueSelector<Var>,
{
    pub fn new(var_selector: VariableSelect, val_selector: ValueSelect) -> Self {
        IndependentVariableValueBrancher {
            variable_selector: var_selector,
            value_selector: val_selector,
            variable_type: PhantomData,
        }
    }
}

pub type DefaultBrancher =
    IndependentVariableValueBrancher<DomainId, InputOrder<DomainId>, InDomainMin>;

impl DefaultBrancher {
    pub fn default_over_all_variables(solver: &ConstraintSatisfactionSolver) -> DefaultBrancher {
        #[allow(deprecated)]
        let variables: Vec<DomainId> = solver.assignments_integer.get_domains().collect::<Vec<_>>();

        IndependentVariableValueBrancher {
            variable_selector: InputOrder::new(&variables),
            value_selector: InDomainMin {},
            variable_type: PhantomData,
        }
    }
}

impl<Var, VariableSelect, ValueSelect> Brancher
    for IndependentVariableValueBrancher<Var, VariableSelect, ValueSelect>
where
    VariableSelect: VariableSelector<Var>,
    ValueSelect: ValueSelector<Var>,
{
    /// First we select a variable
    ///  - If all variables under consideration are fixed (i.e. `select_variable` return None) then
    ///    we simply return None
    ///  - Otherwise we select a value and return the corresponding literal
    fn next_decision(&mut self, context: &mut SelectionContext) -> Option<IntegerPredicate> {
        self.variable_selector
            .select_variable(context)
            .map(|selected_variable| {
                // We have selected a variable, select a value for the PropositionalVariable
                self.value_selector.select_value(context, selected_variable)
            })
    }

    fn on_conflict(&mut self) {
        self.variable_selector.on_conflict()
    }

    fn on_unassign_integer(&mut self, variable: DomainId, value: i32) {
        self.variable_selector.on_unassign_integer(variable, value);
        self.value_selector.on_unassign_integer(variable, value)
    }

    fn on_appearance_in_conflict_integer(&mut self, variable: DomainId) {
        self.variable_selector
            .on_appearance_in_conflict_integer(variable)
    }

    fn on_solution(&mut self, solution: SolutionReference) {
        self.value_selector.on_solution(solution);
    }
}
