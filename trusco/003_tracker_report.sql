-- TrusCo Tracker: date-range report on captured activity (Captured Time → Report).
--
-- Who can see what:
--   * every member can report on their own captured activity;
--   * the firm's own owners and admins (is_org_admin) can report on any member, or on everyone at
--     once. Deliberately not can_admin_org: TrusCo platform admins must not see a firm's raw activity.
-- Raw activity_blocks stay private under RLS (own rows only); these SECURITY DEFINER functions are
-- the only way to read someone else's activity, and they are read-only.
--
-- Rows are pre-aggregated per person, day, matter, app, document/title, status, tariff and time
-- entry so a long range stays small; the page rolls them up into its breakdowns.

create or replace function public.tracker_activity_report(
  p_org uuid,
  p_from date,
  p_to date,
  p_user uuid default null,                -- null = everyone (owners/admins only)
  p_tz text default 'Africa/Johannesburg'  -- the viewer's time zone, for day boundaries
)
returns table (
  user_id uuid,
  user_name text,
  day date,
  matter_id uuid,
  matter_number text,
  matter_title text,
  client_name text,
  matter_source text,         -- 'assigned' | 'suggested' | null (unassigned)
  app_name text,
  document_path text,
  label text,                 -- file name for documents, else window title, else app name
  status text,                -- pending | approved | discarded
  billing_item_id uuid,       -- the chosen tariff item, else the suggested one
  time_entry_id uuid,
  time_entry_amount numeric,  -- per time entry (one entry can cover several rows: sum distinct ids)
  time_entry_status text,     -- unbilled | billed
  seconds bigint,
  blocks integer,
  max_pages integer
)
language plpgsql
stable
security definer
set search_path = public
as $$
declare
  v_uid uuid := auth.uid();
begin
  if v_uid is null or p_org is null or not public.is_org_member(v_uid, p_org) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  if p_from is null or p_to is null or p_to < p_from then
    raise exception 'invalid range: choose a From date on or before the To date' using errcode = '22023';
  end if;
  if p_to - p_from > 366 then
    raise exception 'invalid range: a report can cover at most one year' using errcode = '22023';
  end if;
  if (p_user is null or p_user <> v_uid) and not public.is_org_admin(v_uid, p_org) then
    raise exception 'Only owners and admins can report on other people' using errcode = '42501';
  end if;
  if not exists (select 1 from pg_timezone_names where name = p_tz) then
    raise exception 'invalid time zone' using errcode = '22023';
  end if;

  return query
  with b as (
    select ab.*,
           coalesce(ab.matter_id, ab.suggested_matter_id) as eff_matter,
           case when ab.matter_id is not null then 'assigned'
                when ab.suggested_matter_id is not null then 'suggested' end as src,
           (ab.started_at at time zone p_tz)::date as local_day,
           case when ab.document_path is not null and ab.document_path <> ''
                then regexp_replace(ab.document_path, '^.*[/\\]', '')
                else coalesce(nullif(ab.window_title, ''), ab.app_name, 'Activity') end as lbl
    from public.activity_blocks ab
    where ab.organization_id = p_org
      and (p_user is null or ab.user_id = p_user)
      and ab.started_at >= (p_from::timestamp at time zone p_tz)
      and ab.started_at < ((p_to + 1)::timestamp at time zone p_tz)
  )
  select b.user_id,
         coalesce(nullif(trim(pr.full_name), ''), pr.email, 'Unknown')::text,
         b.local_day,
         b.eff_matter,
         m.matter_number,
         m.title,
         c.name::text,
         b.src::text,
         b.app_name,
         b.document_path,
         b.lbl::text,
         b.status,
         coalesce(b.billing_item_id, b.suggested_billing_item_id),
         b.time_entry_id,
         te.amount,
         te.status::text,
         sum(b.active_seconds)::bigint,
         count(*)::integer,
         max(b.document_pages)::integer
  from b
  left join public.profiles pr on pr.id = b.user_id
  left join public.matters m on m.id = b.eff_matter and m.organization_id = p_org
  left join public.customers c on c.id = m.client_id
  left join public.time_entries te on te.id = b.time_entry_id and te.organization_id = p_org
  group by b.user_id, pr.full_name, pr.email, b.local_day, b.eff_matter, m.matter_number, m.title, c.name,
           b.src, b.app_name, b.document_path, b.lbl, b.status,
           coalesce(b.billing_item_id, b.suggested_billing_item_id), b.time_entry_id, te.amount, te.status
  order by b.local_day, m.matter_number nulls first, b.lbl;
end;
$$;

-- People the caller may report on: owners/admins get every member, everyone else only themselves.
create or replace function public.tracker_report_people(p_org uuid)
returns table (user_id uuid, name text, role text, is_self boolean)
language plpgsql
stable
security definer
set search_path = public
as $$
declare
  v_uid uuid := auth.uid();
begin
  if v_uid is null or p_org is null or not public.is_org_member(v_uid, p_org) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  return query
  select om.user_id,
         coalesce(nullif(trim(pr.full_name), ''), pr.email, 'Unknown')::text,
         om.role,
         om.user_id = v_uid
  from public.organization_members om
  left join public.profiles pr on pr.id = om.user_id
  where om.organization_id = p_org
    and (om.user_id = v_uid or public.is_org_admin(v_uid, p_org))
  order by om.user_id <> v_uid, 2;
end;
$$;

revoke execute on function public.tracker_activity_report(uuid, date, date, uuid, text) from public, anon;
revoke execute on function public.tracker_report_people(uuid) from public, anon;
grant execute on function public.tracker_activity_report(uuid, date, date, uuid, text) to authenticated;
grant execute on function public.tracker_report_people(uuid) to authenticated;
