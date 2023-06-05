use crate::basic_types::Solution;

/// The result of calling [`OptimisationSolver::solve()`].
pub enum OptimisationResult {
    /// There exists no solution with a better objective value than this one.
    Optimal {
        solution: Solution,
        objective_value: u64,
    },
    /// The optimal solution was not found within the time budget. However, at least one solution
    /// was found. The provided solution is the solution with the best objective value that was
    /// encountered.
    Satisfiable {
        best_solution: Solution,
        objective_value: u64,
    },
    /// No solutions exist to the constraint satisfaction problem.
    Infeasible,
    /// No solution was found within the time budget.
    Unknown,
}
