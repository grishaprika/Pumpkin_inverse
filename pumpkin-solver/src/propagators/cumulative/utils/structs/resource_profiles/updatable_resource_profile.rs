use std::marker::PhantomData;
use std::rc::Rc;

use super::ResourceProfileInterface;
use crate::propagators::Task;
use crate::variables::IntegerVariable;

pub(crate) struct UpdatableResourceProfile<Var, Profile: ResourceProfileInterface<Var>> {
    resource_profile: Profile,
    updated: bool,
    variable_type: PhantomData<Var>,
}

impl<Var: IntegerVariable + 'static, Profile: ResourceProfileInterface<Var>>
    ResourceProfileInterface<Var> for UpdatableResourceProfile<Var, Profile>
{
    fn get_start(&self) -> i32 {
        self.resource_profile.get_start()
    }

    fn get_end(&self) -> i32 {
        self.resource_profile.get_end()
    }

    fn get_height(&self) -> i32 {
        self.resource_profile.get_height()
    }

    fn get_profile_tasks(&self) -> &Vec<Rc<Task<Var>>> {
        self.resource_profile.get_profile_tasks()
    }

    fn get_profile_tasks_mut(&mut self) -> &mut Vec<Rc<Task<Var>>> {
        self.resource_profile.get_profile_tasks_mut()
    }

    fn is_updated(&self) -> bool {
        self.updated
    }

    fn mark_updated(&mut self) {
        self.updated = true;
    }

    fn mark_processed(&mut self) {
        self.updated = false;
    }

    fn add_to_height(&mut self, addition: i32) {
        self.resource_profile.add_to_height(addition)
    }

    fn add_profile_task(&mut self, task: Rc<Task<Var>>) {
        self.resource_profile.add_profile_task(task)
    }

    fn remove_profile_task(&mut self, task: &Rc<Task<Var>>) {
        self.resource_profile.remove_profile_task(task)
    }
}
