# Managed room authorization

The authorization state is the authority for managed rooms. A missing state is
fail-closed: it is not evidence of membership. UI rosters, invitations, and
capability advertisements are projections and cannot grant access.

## Role and action matrix

Roles are ordered Guest < Member < Moderator < Owner. An actor may manage only
a target with a strictly lower role, and never the owner.

| Action | Guest | Member | Moderator | Owner |
|---|---:|---:|---:|---:|
| SendMessages / PinMessages / ScreenShare | use assigned permission | use assigned permission | use assigned permission | use assigned permission |
| Grant/Revoke PinMessages, Invite, Kick, Ban | deny | deny | lower-role targets only | non-owner targets |
| Grant/Revoke other permissions | deny | deny | deny | non-owner targets |
| ChangeRole | deny | deny | deny | lower-role, non-owner targets; never Owner |
| Ban / Unban | deny | deny | lower-role targets only | non-owner targets |

A ban denies all capabilities. Unban is valid only for an existing banned
member. An unknown target is never implicitly admitted.

## Invariants

* There is exactly one member with role Owner, and it is `owner`.
* The owner cannot be banned, demoted, or replaced.
* Every member has exactly one permission set and every permission set belongs
a member.
* Authorization events have the current version, matching group, valid
signature/event id, strictly contiguous sequence, and are applied once.
* Evaluation is pure. Mutation occurs only after evaluation succeeds; failed or
denied events therefore leave the logical state unchanged.
* Persisted state is validated before it is restored.

`AuthorizationState::evaluate` is the typed fail-closed policy boundary;
`AuthorizationState::apply` is the atomic transition boundary.
