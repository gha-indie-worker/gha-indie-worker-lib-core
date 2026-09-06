import gha_indie_worker_lib_core.{
  Activated, Active, Advanced, AlreadyThere, BillingWasLinked, Created,
  FirstWorkflowWasPlanned, GithubAppWasInstalled, ProfileCompleted, Rejected,
  UnknownEvent, WasSuspended,
}
import gleam/list
import gleeunit
import gleeunit/should

pub fn main() {
  gleeunit.main()
}

pub fn org_edges_are_deterministic_test() {
  gha_indie_worker_lib_core.org_transitions()
  |> list.each(fn(edge) {
    gha_indie_worker_lib_core.org_transitions()
    |> list.filter(fn(other) { other.0 == edge.0 && other.1 == edge.1 })
    |> list.length
    |> should.equal(1)
  })
}

pub fn no_self_loops_test() {
  gha_indie_worker_lib_core.org_transitions()
  |> list.each(fn(edge) { should.be_false(edge.0 == edge.2) })
}

pub fn happy_path_reaches_active_test() {
  let path = [
    ProfileCompleted,
    BillingWasLinked,
    GithubAppWasInstalled,
    FirstWorkflowWasPlanned,
    Activated,
  ]
  let final =
    list.fold(path, Created, fn(state, event) {
      case gha_indie_worker_lib_core.advance_org(state, event) {
        Advanced(next) -> next
        _ -> state
      }
    })
  should.equal(final, Active)
  should.be_true(gha_indie_worker_lib_core.org_may_run_workflows(final))
}

pub fn replay_is_idempotent_test() {
  gha_indie_worker_lib_core.advance_org(
    gha_indie_worker_lib_core.OrgProfile,
    ProfileCompleted,
  )
  |> should.equal(AlreadyThere(gha_indie_worker_lib_core.OrgProfile))
}

pub fn foreign_event_is_unknown_test() {
  gha_indie_worker_lib_core.advance_org(
    Created,
    gha_indie_worker_lib_core.EmailWasVerified,
  )
  |> should.equal(Rejected(UnknownEvent))
}

pub fn suspension_reaches_every_state_test() {
  gha_indie_worker_lib_core.org_states()
  |> list.filter(fn(state) { state != gha_indie_worker_lib_core.Suspended })
  |> list.each(fn(state) {
    case gha_indie_worker_lib_core.advance_org(state, WasSuspended) {
      Advanced(_) -> Nil
      _ -> should.fail()
    }
  })
}

pub fn wire_values_match_the_contract_test() {
  gha_indie_worker_lib_core.org_state_to_wire(
    gha_indie_worker_lib_core.GithubAppInstalled,
  )
  |> should.equal("github-app-installed")
}
