-- TrusCo Tracker tasks: project tasks with a due time, a type (to-do / allocate time),
-- a priority flag, an optional time slot and an owner, managed from the desktop tracker.
-- Builds on the existing public.project_tasks so tasks also show on the project in TrusCo.

-- ---------------------------------------------------------------------------
-- 1. New columns on project_tasks (existing rows keep working: all optional / defaulted)
-- ---------------------------------------------------------------------------
alter table public.project_tasks
  add column if not exists due_at timestamptz,
  add column if not exists kind text not null default 'todo',
  add column if not exists priority text not null default 'normal',
  add column if not exists scheduled_at timestamptz,
  add column if not exists assignee_user_id uuid references auth.users(id) on delete set null,
  add column if not exists completed_at timestamptz;

alter table public.project_tasks
  add constraint project_tasks_kind_check check (kind in ('todo', 'time')),
  add constraint project_tasks_priority_check check (priority in ('normal', 'high'));

create index if not exists project_tasks_assignee_idx
  on public.project_tasks (assignee_user_id, status) where assignee_user_id is not null;
create index if not exists project_tasks_created_by_idx on public.project_tasks (created_by, status);

-- Stamp completed_at whenever a task is marked done (from the tracker or the web).
create or replace function public.project_tasks_stamp_completed()
returns trigger
language plpgsql
set search_path = public
as $$
begin
  if new.status = 'done' then
    if tg_op = 'INSERT' or old.status is distinct from 'done' then
      new.completed_at := coalesce(new.completed_at, now());
    end if;
  else
    new.completed_at := null;
  end if;
  return new;
end;
$$;

create trigger project_tasks_stamp_completed
  before insert or update on public.project_tasks
  for each row execute function public.project_tasks_stamp_completed();

-- ---------------------------------------------------------------------------
-- 2. Helpers
-- ---------------------------------------------------------------------------
-- The organization and user a paired, non-revoked device acts for.
create or replace function public.tracker_device_owner(p_device_id uuid, out organization_id uuid, out user_id uuid)
language plpgsql stable security definer set search_path = public
as $$
begin
  select d.organization_id, d.user_id into organization_id, user_id
  from public.tracker_devices d
  where d.id = p_device_id
    and d.revoked_at is null
    and d.token_hash is not null
    and exists (select 1 from public.organization_members om
                where om.organization_id = d.organization_id and om.user_id = d.user_id);
  if organization_id is null then
    raise exception 'invalid device';
  end if;
end;
$$;

-- Write access for a device's user. can_write_org_u() needs a signed-in session (auth.uid()),
-- which server-side device calls don't have, so check the member's role directly.
create or replace function public.tracker_device_can_write(p_org uuid, p_user uuid)
returns boolean
language sql stable security definer set search_path = public
as $$
  select exists (select 1 from public.organization_members
                 where organization_id = p_org and user_id = p_user and role in ('owner', 'admin', 'member'));
$$;

create or replace function public.tracker_task_json(p_task_id uuid)
returns jsonb
language sql stable security definer set search_path = public
as $$
  select jsonb_build_object(
    'id', t.id,
    'project_id', t.project_id,
    'project_number', p.project_number,
    'project_name', p.name,
    'title', t.title,
    'kind', t.kind,
    'priority', t.priority,
    'status', t.status,
    'due_at', t.due_at,
    'due_date', t.due_date,
    'scheduled_at', t.scheduled_at,
    'estimated_hours', t.estimated_hours,
    'completed_at', t.completed_at,
    'updated_at', t.updated_at)
  from public.project_tasks t
  join public.projects p on p.id = t.project_id
  where t.id = p_task_id;
$$;

-- ---------------------------------------------------------------------------
-- 3. Device-facing functions (called by /api/public/tracker/* with the service role)
-- ---------------------------------------------------------------------------
create or replace function public.tracker_list_projects(p_device_id uuid)
returns jsonb
language plpgsql stable security definer set search_path = public
as $$
declare
  o record;
begin
  select * into o from public.tracker_device_owner(p_device_id);
  return coalesce((
    select jsonb_agg(jsonb_build_object('id', p.id, 'number', p.project_number, 'name', p.name, 'client', c.name)
                     order by p.created_at desc)
    from public.projects p
    left join public.customers c on c.id = p.client_id
    where p.organization_id = o.organization_id
      and p.archived_at is null
      and coalesce(p.status, 'active') not in ('completed', 'cancelled', 'closed', 'archived')
  ), '[]'::jsonb);
end;
$$;

create or replace function public.tracker_create_project(p_device_id uuid, p_name text)
returns jsonb
language plpgsql security definer set search_path = public
as $$
declare
  o record;
  v_name text := btrim(coalesce(p_name, ''));
  v_prefix text;
  v_next int;
  v_number text;
  v_id uuid;
begin
  select * into o from public.tracker_device_owner(p_device_id);
  if not public.tracker_device_can_write(o.organization_id, o.user_id) then
    raise exception 'not allowed to create projects in this organization';
  end if;
  if length(v_name) < 2 or length(v_name) > 120 then
    raise exception 'invalid project: name must be 2 to 120 characters';
  end if;

  -- Same numbering as the web app: organization prefix + counter (row lock avoids duplicates).
  select coalesce(project_number_prefix, 'PRJ-'), coalesce(project_next_number, 1001)
    into v_prefix, v_next
  from public.organizations where id = o.organization_id
  for update;
  loop
    v_number := v_prefix || v_next;
    exit when not exists (select 1 from public.projects
                          where organization_id = o.organization_id and project_number = v_number);
    v_next := v_next + 1;
  end loop;

  insert into public.projects (organization_id, user_id, project_number, name)
  values (o.organization_id, o.user_id, v_number, v_name)
  returning id into v_id;
  update public.organizations set project_next_number = v_next + 1 where id = o.organization_id;

  return jsonb_build_object('id', v_id, 'number', v_number, 'name', v_name, 'client', null);
end;
$$;

-- The user's tasks: those assigned to them (the tracker assigns new tasks to their creator; the
-- web assigns via the "Assigned to" picker). Unassigned project template tasks are left out.
-- Open tasks plus anything completed in the last 24 hours.
create or replace function public.tracker_list_tasks(p_device_id uuid)
returns jsonb
language plpgsql stable security definer set search_path = public
as $$
declare
  o record;
begin
  select * into o from public.tracker_device_owner(p_device_id);
  return coalesce((
    select jsonb_agg(public.tracker_task_json(t.id)
                     order by (t.status = 'done'), coalesce(t.due_at, t.due_date::timestamptz) nulls last,
                              (t.priority = 'high') desc, t.created_at)
    from (
      select t.*
      from public.project_tasks t
      join public.projects p on p.id = t.project_id
      where t.organization_id = o.organization_id
        and p.archived_at is null
        and t.assignee_user_id = o.user_id
        and (t.status <> 'done' or t.completed_at > now() - interval '24 hours')
      order by (t.status = 'done'), coalesce(t.due_at, t.due_date::timestamptz) nulls last, t.created_at
      limit 300
    ) t
  ), '[]'::jsonb);
end;
$$;

-- Create (no "id") or update (with "id") a task. On update only the keys present change.
-- Keys: id, project_id, title, kind, priority, status, due_at, due_date, scheduled_at, estimated_hours.
create or replace function public.tracker_save_task(p_device_id uuid, p_task jsonb)
returns jsonb
language plpgsql security definer set search_path = public
as $$
declare
  o record;
  t record;
  v_id uuid;
  v_project uuid;
  v_title text;
  v_kind text;
  v_priority text;
  v_status text;
  v_due_at timestamptz;
  v_due_date date;
  v_scheduled timestamptz;
  v_hours numeric;
  v_assignee text;
begin
  select * into o from public.tracker_device_owner(p_device_id);
  if not public.tracker_device_can_write(o.organization_id, o.user_id) then
    raise exception 'not allowed to manage tasks in this organization';
  end if;
  if p_task is null or jsonb_typeof(p_task) <> 'object' then
    raise exception 'invalid task: expected an object';
  end if;

  begin
    v_id        := nullif(p_task->>'id', '')::uuid;
    v_project   := nullif(p_task->>'project_id', '')::uuid;
    v_due_at    := nullif(p_task->>'due_at', '')::timestamptz;
    v_due_date  := nullif(p_task->>'due_date', '')::date;
    v_scheduled := nullif(p_task->>'scheduled_at', '')::timestamptz;
    v_hours     := nullif(p_task->>'estimated_hours', '')::numeric;
  exception when others then
    raise exception 'invalid task: %', sqlerrm;
  end;
  v_title    := btrim(p_task->>'title');
  v_kind     := p_task->>'kind';
  v_priority := p_task->>'priority';
  v_status   := p_task->>'status';

  if p_task ? 'title' and (v_title is null or length(v_title) < 1 or length(v_title) > 300) then
    raise exception 'invalid task: title must be 1 to 300 characters';
  end if;
  if p_task ? 'kind' and v_kind not in ('todo', 'time') then
    raise exception 'invalid task: kind must be todo or time';
  end if;
  if p_task ? 'priority' and v_priority not in ('normal', 'high') then
    raise exception 'invalid task: priority must be normal or high';
  end if;
  if p_task ? 'status' and v_status not in ('todo', 'in_progress', 'done') then
    raise exception 'invalid task: status must be todo, in_progress or done';
  end if;
  if v_hours is not null and (v_hours < 0 or v_hours > 1000) then
    raise exception 'invalid task: estimated_hours must be between 0 and 1000';
  end if;
  -- A due time without a date: derive the South African calendar date.
  if v_due_at is not null and v_due_date is null then
    v_due_date := (v_due_at at time zone 'Africa/Johannesburg')::date;
  end if;
  if v_project is not null and not exists (
    select 1 from public.projects where id = v_project and organization_id = o.organization_id and archived_at is null
  ) then
    raise exception 'invalid task: project not found';
  end if;

  if v_id is null then
    if v_project is null or v_title is null then
      raise exception 'invalid task: project_id and title are required';
    end if;
    select coalesce(nullif(btrim(full_name), ''), email) into v_assignee from public.profiles where id = o.user_id;
    insert into public.project_tasks
      (organization_id, project_id, title, kind, priority, status, due_at, due_date, scheduled_at,
       estimated_hours, position, created_by, assignee_user_id, assignee)
    values
      (o.organization_id, v_project, v_title, coalesce(v_kind, 'todo'), coalesce(v_priority, 'normal'),
       coalesce(v_status, 'todo'), v_due_at, v_due_date, v_scheduled, coalesce(v_hours, 0),
       coalesce((select max(position) + 1 from public.project_tasks where project_id = v_project), 0),
       o.user_id, o.user_id, v_assignee)
    returning id into v_id;
  else
    select * into t from public.project_tasks
    where id = v_id and organization_id = o.organization_id
      and (assignee_user_id = o.user_id or (assignee_user_id is null and created_by = o.user_id));
    if not found then
      raise exception 'invalid task: task not found';
    end if;
    update public.project_tasks set
      project_id      = case when p_task ? 'project_id' and v_project is not null then v_project else project_id end,
      title           = case when p_task ? 'title' then v_title else title end,
      kind            = case when p_task ? 'kind' then v_kind else kind end,
      priority        = case when p_task ? 'priority' then v_priority else priority end,
      status          = case when p_task ? 'status' then v_status else status end,
      due_at          = case when p_task ? 'due_at' then v_due_at else due_at end,
      due_date        = case when p_task ? 'due_date' or p_task ? 'due_at' then v_due_date else due_date end,
      scheduled_at    = case when p_task ? 'scheduled_at' then v_scheduled else scheduled_at end,
      estimated_hours = case when p_task ? 'estimated_hours' then coalesce(v_hours, 0) else estimated_hours end
    where id = v_id;
  end if;

  return public.tracker_task_json(v_id);
end;
$$;

-- ---------------------------------------------------------------------------
-- 4. Grants: only the server (service role) may call the device functions
-- ---------------------------------------------------------------------------
revoke execute on function public.tracker_device_owner(uuid) from public, anon, authenticated;
revoke execute on function public.tracker_device_can_write(uuid, uuid) from public, anon, authenticated;
revoke execute on function public.tracker_task_json(uuid) from public, anon, authenticated;
revoke execute on function public.tracker_list_projects(uuid) from public, anon, authenticated;
revoke execute on function public.tracker_create_project(uuid, text) from public, anon, authenticated;
revoke execute on function public.tracker_list_tasks(uuid) from public, anon, authenticated;
revoke execute on function public.tracker_save_task(uuid, jsonb) from public, anon, authenticated;
grant execute on function public.tracker_device_owner(uuid) to service_role;
grant execute on function public.tracker_device_can_write(uuid, uuid) to service_role;
grant execute on function public.tracker_task_json(uuid) to service_role;
grant execute on function public.tracker_list_projects(uuid) to service_role;
grant execute on function public.tracker_create_project(uuid, text) to service_role;
grant execute on function public.tracker_list_tasks(uuid) to service_role;
grant execute on function public.tracker_save_task(uuid, jsonb) to service_role;
