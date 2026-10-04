-- Trusco Tracker: desktop activity capture -> review inbox -> time_entries
-- Tables: tracker_devices, activity_blocks, matter_aliases
-- Functions: tracker_match_matter, tracker_suggest_billing_item, tracker_ingest_blocks (service_role only),
--            create_tracker_pairing_code, revoke_tracker_device, approve_activity_blocks, tracker_rematch_my_pending

-- ---------------------------------------------------------------------------
-- 1. Paired desktop/mobile devices (one row per device; token stored hashed)
-- ---------------------------------------------------------------------------
create table public.tracker_devices (
  id uuid primary key default gen_random_uuid(),
  organization_id uuid not null references public.organizations(id) on delete cascade,
  user_id uuid not null references auth.users(id) on delete cascade,
  name text not null default 'New device',
  platform text,
  app_version text,
  pairing_code_hash text,
  pairing_expires_at timestamptz,
  token_hash text unique,
  paired_at timestamptz,
  last_seen_at timestamptz,
  revoked_at timestamptz,
  created_at timestamptz not null default now()
);
create index tracker_devices_pairing_idx on public.tracker_devices (pairing_code_hash) where pairing_code_hash is not null;
create index tracker_devices_user_idx on public.tracker_devices (user_id);

alter table public.tracker_devices enable row level security;
create policy "tracker_devices own read" on public.tracker_devices
  for select to authenticated using (user_id = auth.uid());
create policy "tracker_devices own delete" on public.tracker_devices
  for delete to authenticated using (user_id = auth.uid());
-- inserts/updates only through create_tracker_pairing_code / revoke_tracker_device / edge functions

-- ---------------------------------------------------------------------------
-- 2. Captured activity awaiting review (private to the user who captured it)
-- ---------------------------------------------------------------------------
create table public.activity_blocks (
  id uuid primary key default gen_random_uuid(),
  organization_id uuid not null references public.organizations(id) on delete cascade,
  user_id uuid not null references auth.users(id) on delete cascade,
  device_id uuid references public.tracker_devices(id) on delete set null,
  external_id text not null,
  source text not null default 'desktop'
    check (source in ('desktop','email','call','whatsapp','mobile','manual')),
  started_at timestamptz not null,
  ended_at timestamptz not null,
  active_seconds integer not null default 0 check (active_seconds >= 0),
  app_name text,
  app_bundle text,
  window_title text,
  document_path text,
  document_pages integer,
  url text,
  participants jsonb,
  suggested_matter_id uuid references public.matters(id) on delete set null,
  match_score integer,
  match_reason text,
  suggested_billing_item_id uuid references public.billing_items(id) on delete set null,
  matter_id uuid references public.matters(id) on delete set null,
  billing_item_id uuid references public.billing_items(id) on delete set null,
  narrative text,
  status text not null default 'pending' check (status in ('pending','approved','discarded')),
  time_entry_id uuid references public.time_entries(id) on delete set null,
  reviewed_at timestamptz,
  reviewed_by uuid,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  constraint activity_blocks_external_uniq unique (user_id, source, external_id)
);
create index activity_blocks_user_status_idx on public.activity_blocks (user_id, status, started_at desc);
create index activity_blocks_org_idx on public.activity_blocks (organization_id, started_at desc);

create trigger set_activity_blocks_updated_at before update on public.activity_blocks
  for each row execute function public.set_updated_at();

alter table public.activity_blocks enable row level security;
create policy "activity_blocks own read" on public.activity_blocks
  for select to authenticated
  using (user_id = auth.uid() and public.is_org_member(auth.uid(), organization_id));
create policy "activity_blocks own update" on public.activity_blocks
  for update to authenticated
  using (user_id = auth.uid() and public.is_org_member(auth.uid(), organization_id))
  with check (user_id = auth.uid() and public.is_org_member(auth.uid(), organization_id));
create policy "activity_blocks own delete" on public.activity_blocks
  for delete to authenticated
  using (user_id = auth.uid());
-- inserts only through tracker_ingest_blocks (service role)

-- ---------------------------------------------------------------------------
-- 3. Matching rules: "anything with this keyword / under this folder belongs to matter X"
-- ---------------------------------------------------------------------------
create table public.matter_aliases (
  id uuid primary key default gen_random_uuid(),
  organization_id uuid not null references public.organizations(id) on delete cascade,
  matter_id uuid not null references public.matters(id) on delete cascade,
  kind text not null check (kind in ('keyword','path','email','domain','phone')),
  pattern text not null check (length(btrim(pattern)) >= 3),
  created_by uuid,
  created_at timestamptz not null default now()
);
create unique index matter_aliases_uniq on public.matter_aliases (organization_id, kind, lower(pattern));
create index matter_aliases_matter_idx on public.matter_aliases (matter_id);

alter table public.matter_aliases enable row level security;
create policy "matter_aliases org read" on public.matter_aliases
  for select to authenticated using (public.is_org_member(auth.uid(), organization_id));
create policy "matter_aliases org write" on public.matter_aliases
  for all to authenticated
  using (public.can_write_org_u(auth.uid(), organization_id))
  with check (public.can_write_org_u(auth.uid(), organization_id));

-- ---------------------------------------------------------------------------
-- 4. Matching: which open matter does a window title / file path belong to?
--    Priority: rule (100) > matter number (95) > client full name (80) > client surname (60);
--    +5 when a word from the matter title also appears (separates a client's matters).
-- ---------------------------------------------------------------------------
create or replace function public.tracker_match_matter(p_org uuid, p_title text, p_path text)
returns table (matter_id uuid, score integer, reason text)
language plpgsql stable security definer set search_path = public
as $$
declare
  v_raw   text := replace(lower(coalesce(p_title, '') || ' ' || coalesce(p_path, '')), '\', '/');
  v_norm  text;
  v_path  text := replace(lower(coalesce(p_path, '')), '\', '/');
  r record;
  part text;
  full_name text;
  surname text;
  w text;
  s int;
  why text;
  best_id uuid;
  best_score int := 0;
  best_reason text;
  best_len int := 0;
  best_updated timestamptz;
  ties int := 0;
  stop text[] := array['the','and','van','von','der','den','pty','ltd','inc','cc','trust','estate','family',
    'holdings','company','attorneys','client','group','test','divorce','will','late','matter','new','draft','final'];
begin
  if p_org is null or btrim(v_raw) = '' then
    return;
  end if;
  v_norm := ' ' || btrim(regexp_replace(v_raw, '[[:space:][:punct:]–—’‘“”]+', ' ', 'g')) || ' ';

  -- 4a. explicit rules (longest pattern wins)
  for r in
    select a.matter_id as mid, a.kind, replace(lower(btrim(a.pattern)), '\', '/') as pat
    from public.matter_aliases a
    join public.matters m on m.id = a.matter_id
    where a.organization_id = p_org
      and m.organization_id = p_org
      and coalesce(m.status, 'open') not in ('closed', 'archived')
      and a.kind in ('keyword', 'path')
  loop
    if (r.kind = 'path' and v_path <> '' and position(r.pat in v_path) > 0)
       or (r.kind = 'keyword' and position(r.pat in v_raw) > 0) then
      if length(r.pat) > best_len then
        best_len := length(r.pat);
        best_id := r.mid;
        best_score := 100;
        best_reason := 'Rule: ' || r.kind || ' "' || r.pat || '"';
      end if;
    end if;
  end loop;
  if best_score = 100 then
    return query select best_id, best_score, best_reason;
    return;
  end if;

  -- 4b. matter number / client name
  for r in
    select m.id, m.matter_number, m.title, c.name as client_name, m.updated_at
    from public.matters m
    left join public.customers c on c.id = m.client_id
    where m.organization_id = p_org
      and coalesce(m.status, 'open') not in ('closed', 'archived')
  loop
    s := 0;
    why := null;

    if r.matter_number is not null and length(r.matter_number) >= 3
       and v_raw ~ ('(^|[^[:alnum:]])'
                    || regexp_replace(lower(r.matter_number), '[^[:alnum:]]+', '[^[:alnum:]]?', 'g')
                    || '($|[^[:alnum:]])') then
      s := 95;
      why := 'Matter number ' || r.matter_number;
    elsif r.client_name is not null then
      foreach part in array regexp_split_to_array(lower(r.client_name), '\s*(/|&|,|\+|\sand\s|\sen\s)\s*') loop
        full_name := btrim(regexp_replace(part, '[[:space:][:punct:]–—’‘“”]+', ' ', 'g'));
        continue when length(full_name) < 3;
        if position(' ' in full_name) > 0 and position(' ' || full_name || ' ' in v_norm) > 0 then
          if s < 80 then s := 80; why := 'Client name "' || full_name || '"'; end if;
        else
          surname := (regexp_split_to_array(full_name, ' '))[array_length(regexp_split_to_array(full_name, ' '), 1)];
          if length(surname) >= 4 and not (surname = any(stop))
             and position(' ' || surname || ' ' in v_norm) > 0 and s < 60 then
            s := 60;
            why := 'Client name "' || surname || '"';
          end if;
        end if;
      end loop;
    end if;

    if s > 0 and s < 95 and r.title is not null then
      foreach w in array regexp_split_to_array(lower(r.title), '[[:space:][:punct:]–—’‘“”]+') loop
        if length(w) >= 4 and not (w = any(stop)) and position(' ' || w || ' ' in v_norm) > 0 then
          s := s + 5;
          why := why || ' + "' || w || '"';
          exit;
        end if;
      end loop;
    end if;

    if s > best_score
       or (s = best_score and s > 0 and r.updated_at > best_updated) then
      if s = best_score then ties := ties + 1; else ties := 0; end if;
      best_score := s;
      best_id := r.id;
      best_reason := why;
      best_updated := r.updated_at;
    elsif s = best_score and s > 0 then
      ties := ties + 1;
    end if;
  end loop;

  if best_score > 0 then
    if ties > 0 then
      best_reason := best_reason || ' (also matches ' || ties || ' other open matter'
                     || case when ties > 1 then 's' else '' end || ')';
      best_score := best_score - 10;
    end if;
    return query select best_id, best_score, best_reason;
  end if;
end;
$$;

-- ---------------------------------------------------------------------------
-- 5. Tariff suggestion from the org's own billing_items
-- ---------------------------------------------------------------------------
create or replace function public.tracker_suggest_billing_item(p_org uuid, p_app text, p_title text, p_path text)
returns uuid
language plpgsql stable security definer set search_path = public
as $$
declare
  v_app  text := lower(coalesce(p_app, ''));
  v_raw  text := lower(coalesce(p_title, '') || ' ' || coalesce(p_path, ''));
  v_norm text;
  v_kind text;
  v_id   uuid;
  r record;
  w text;
  ov int;
  best_ov int := 0;
  doc_words text[] := array['settlement','parenting','plan','testament','will','affidavit','founding','answering',
    'replying','summons','heads','argument','notice','motion','application','letter','demand','contract',
    'agreement','plea','exception','order','maintenance','protection','subpoena','memo','brief','index',
    'discovery','donation','deed','power','attorney','consent','reply'];
begin
  if p_org is null then
    return null;
  end if;
  v_norm := ' ' || btrim(regexp_replace(v_raw, '[[:space:][:punct:]–—’‘“”]+', ' ', 'g')) || ' ';

  v_kind := case
    when v_app ~ '^(zoom|webex|facetime|google meet)' or v_raw ~ '(meet\.google|google meet|zoom meeting)'
         or (v_app ~ 'teams' and v_raw ~ '(meeting|call)') then 'meeting'
    when v_app ~ '^(microsoft outlook|outlook|mail|thunderbird|spark|airmail|postbox|mailspring|canary)'
         or v_raw ~ '(gmail|outlook\.(com|office)|- outlook)' then 'email'
    when v_raw ~ '\.pdf([^[:alnum:]]|$)' or v_app ~ '^(adobe acrobat|acrobat|preview|pdf expert|foxit|nitro)' then 'pdf'
    when v_app ~ '^(microsoft word|word|pages|libreoffice|wps)'
         or v_raw ~ '(\.docx?|\.odt|\.rtf)([^[:alnum:]]|$)' or v_raw ~ 'google docs' then 'drafting'
    else 'general'
  end;

  if v_kind = 'drafting' then
    for r in
      select id, name from public.billing_items
      where organization_id = p_org and active and (category ilike 'drafting%' or name ilike 'drafting%')
      order by length(name)
    loop
      ov := 0;
      foreach w in array regexp_split_to_array(lower(r.name), '[[:space:][:punct:]]+') loop
        if w = any(doc_words) and position(' ' || w || ' ' in v_norm) > 0 then
          ov := ov + 1;
        end if;
      end loop;
      if ov > best_ov then
        best_ov := ov;
        v_id := r.id;
      end if;
    end loop;
    if v_id is null then
      select id into v_id from public.billing_items
      where organization_id = p_org and active and name ilike 'drafting of document%' limit 1;
    end if;
    if v_id is null then
      select id into v_id from public.billing_items
      where organization_id = p_org and active and (category ilike 'drafting%' or name ilike 'drafting%')
      order by length(name) limit 1;
    end if;
  elsif v_kind = 'email' then
    select id into v_id from public.billing_items
    where organization_id = p_org and active
      and (name ilike '%perusal of correspondence%' or name ilike '%email%' or name ilike '%correspondence%')
    order by (name ilike '%perusal%') desc, length(name) limit 1;
  elsif v_kind = 'pdf' then
    select id into v_id from public.billing_items
    where organization_id = p_org and active and name ilike 'perusal%'
    order by (name ilike '%pleadings%') desc, length(name) limit 1;
  elsif v_kind = 'meeting' then
    select id into v_id from public.billing_items
    where organization_id = p_org and active
      and (name ilike '%meeting attendance%' or name ilike '%telephone consultation%' or name ilike 'consultation%')
    order by (name ilike '%meeting%') desc, length(name) limit 1;
  end if;

  if v_id is null then
    select id into v_id from public.billing_items
    where organization_id = p_org and active and name ilike '%general attendance%' limit 1;
  end if;
  return v_id;
end;
$$;

-- ---------------------------------------------------------------------------
-- 6. Ingest from a paired device (called by the tracker-sync edge function only)
-- ---------------------------------------------------------------------------
create or replace function public.tracker_ingest_blocks(p_device_id uuid, p_blocks jsonb)
returns jsonb
language plpgsql security definer set search_path = public
as $$
declare
  d record;
  b jsonb;
  v_ext text;
  v_started timestamptz;
  v_ended timestamptz;
  v_secs int;
  v_pages int;
  v_title text;
  v_path text;
  v_app text;
  v_existing record;
  m record;
  v_bi uuid;
  v_id uuid;
  v_matter uuid;
  v_status text;
  v_label text;
  v_results jsonb := '[]'::jsonb;
begin
  select * into d from public.tracker_devices
  where id = p_device_id and revoked_at is null and token_hash is not null;
  if not found then
    raise exception 'invalid device';
  end if;
  if not exists (select 1 from public.organization_members om
                 where om.organization_id = d.organization_id and om.user_id = d.user_id) then
    raise exception 'device user is no longer a member of the organization';
  end if;

  for b in select value from jsonb_array_elements(coalesce(p_blocks, '[]'::jsonb)) limit 500 loop
    v_ext := left(b->>'external_id', 100);
    continue when v_ext is null or v_ext = '';
    begin
      v_started := (b->>'started_at')::timestamptz;
      v_ended   := (b->>'ended_at')::timestamptz;
      v_secs    := least(greatest(coalesce((b->>'active_seconds')::numeric, 0)::int, 0), 86400);
      v_pages   := nullif(b->>'document_pages', '')::int;
    exception when others then
      continue;
    end;
    continue when v_started is null or v_ended is null or v_ended < v_started;
    if v_pages is not null and (v_pages < 0 or v_pages > 100000) then v_pages := null; end if;

    v_title := left(b->>'window_title', 500);
    v_path  := left(b->>'document_path', 1000);
    v_app   := left(b->>'app_name', 120);

    select ab.id, ab.status into v_existing
    from public.activity_blocks ab
    where ab.user_id = d.user_id and ab.source = 'desktop' and ab.external_id = v_ext;
    if found and v_existing.status <> 'pending' then
      v_results := v_results || jsonb_build_object('external_id', v_ext, 'status', v_existing.status);
      continue;
    end if;

    select * into m from public.tracker_match_matter(d.organization_id, v_title, v_path) limit 1;
    v_bi := public.tracker_suggest_billing_item(d.organization_id, v_app, v_title, v_path);

    insert into public.activity_blocks as ab
      (organization_id, user_id, device_id, external_id, source, started_at, ended_at, active_seconds,
       app_name, app_bundle, window_title, document_path, document_pages, url,
       suggested_matter_id, match_score, match_reason, suggested_billing_item_id)
    values
      (d.organization_id, d.user_id, d.id, v_ext, 'desktop', v_started, v_ended, v_secs,
       v_app, left(b->>'app_bundle', 200), v_title, v_path, v_pages, left(b->>'url', 1000),
       m.matter_id, m.score, m.reason, v_bi)
    on conflict (user_id, source, external_id) do update set
      started_at = least(ab.started_at, excluded.started_at),
      ended_at = greatest(ab.ended_at, excluded.ended_at),
      active_seconds = greatest(ab.active_seconds, excluded.active_seconds),
      window_title = coalesce(excluded.window_title, ab.window_title),
      document_path = coalesce(excluded.document_path, ab.document_path),
      document_pages = coalesce(excluded.document_pages, ab.document_pages),
      url = coalesce(excluded.url, ab.url),
      suggested_matter_id = excluded.suggested_matter_id,
      match_score = excluded.match_score,
      match_reason = excluded.match_reason,
      suggested_billing_item_id = coalesce(ab.suggested_billing_item_id, excluded.suggested_billing_item_id)
    where ab.status = 'pending'
    returning ab.id, coalesce(ab.matter_id, ab.suggested_matter_id), ab.status
      into v_id, v_matter, v_status;

    v_label := null;
    if v_matter is not null then
      select mm.matter_number || ' · ' || mm.title into v_label from public.matters mm where mm.id = v_matter;
    end if;
    v_results := v_results || jsonb_build_object(
      'external_id', v_ext,
      'status', coalesce(v_status, 'pending'),
      'matter_id', v_matter,
      'matter_label', v_label,
      'match_reason', m.reason);
  end loop;

  update public.tracker_devices set last_seen_at = now() where id = d.id;
  return jsonb_build_object('results', v_results);
end;
$$;

-- ---------------------------------------------------------------------------
-- 7. User-facing RPCs
-- ---------------------------------------------------------------------------
create or replace function public.create_tracker_pairing_code(p_org uuid)
returns text
language plpgsql security definer set search_path = public, extensions
as $$
declare
  alphabet text := 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';  -- 32 chars, no 0/O/1/I
  v_bytes bytea := extensions.gen_random_bytes(8);
  v_code text := '';
  i int;
begin
  if auth.uid() is null then
    raise exception 'not authenticated';
  end if;
  if p_org is null or not public.is_org_member(auth.uid(), p_org) then
    raise exception 'not a member of this organization';
  end if;
  for i in 0..7 loop
    v_code := v_code || substr(alphabet, 1 + (get_byte(v_bytes, i) % 32), 1);
  end loop;
  delete from public.tracker_devices
  where user_id = auth.uid() and token_hash is null and pairing_expires_at < now();
  insert into public.tracker_devices (organization_id, user_id, pairing_code_hash, pairing_expires_at)
  values (p_org, auth.uid(), encode(extensions.digest(v_code, 'sha256'), 'hex'), now() + interval '10 minutes');
  return substr(v_code, 1, 4) || '-' || substr(v_code, 5, 4);
end;
$$;

create or replace function public.revoke_tracker_device(p_device_id uuid)
returns void
language plpgsql security definer set search_path = public
as $$
begin
  update public.tracker_devices
  set revoked_at = now(), token_hash = null, pairing_code_hash = null
  where id = p_device_id and user_id = auth.uid();
end;
$$;

-- Turn one or more pending blocks into a single unbilled time entry.
create or replace function public.approve_activity_blocks(
  p_block_ids uuid[],
  p_matter_id uuid,
  p_billing_item_id uuid,
  p_date date,
  p_basis public.billing_basis,
  p_units numeric,
  p_rate numeric,
  p_narrative text,
  p_remember_kind text default null,
  p_remember_pattern text default null
)
returns uuid
language plpgsql security definer set search_path = public
as $$
declare
  v_org uuid;
  v_cnt int;
  v_category text;
  v_entry uuid;
begin
  if auth.uid() is null then
    raise exception 'not authenticated';
  end if;
  if coalesce(array_length(p_block_ids, 1), 0) = 0 then
    raise exception 'no activity selected';
  end if;
  select organization_id into v_org from public.matters where id = p_matter_id;
  if v_org is null then
    raise exception 'matter not found';
  end if;
  if not public.can_write_org_u(auth.uid(), v_org) then
    raise exception 'not allowed to record time in this organization';
  end if;
  select count(*) into v_cnt from public.activity_blocks
  where id = any(p_block_ids) and user_id = auth.uid() and organization_id = v_org and status = 'pending';
  if v_cnt <> (select count(distinct x) from unnest(p_block_ids) x) then
    raise exception 'some activity is no longer pending or is not yours';
  end if;
  if p_billing_item_id is not null then
    select category into v_category from public.billing_items
    where id = p_billing_item_id and organization_id = v_org;
    if not found then
      raise exception 'tariff item does not belong to this organization';
    end if;
  end if;
  if p_basis is null or p_units is null or p_units <= 0 or p_rate is null or p_rate < 0 then
    raise exception 'units and rate are required';
  end if;

  insert into public.time_entries
    (organization_id, matter_id, date, billing_item_id, category, narrative, basis, units, rate, amount, status, created_by)
  values
    (v_org, p_matter_id, coalesce(p_date, current_date), p_billing_item_id, v_category, coalesce(p_narrative, ''),
     p_basis, p_units, p_rate, round(p_units * p_rate, 2), 'unbilled', auth.uid())
  returning id into v_entry;

  update public.activity_blocks
  set status = 'approved', matter_id = p_matter_id, billing_item_id = p_billing_item_id,
      time_entry_id = v_entry, narrative = p_narrative, reviewed_at = now(), reviewed_by = auth.uid()
  where id = any(p_block_ids);

  if p_remember_kind in ('keyword', 'path') and length(btrim(coalesce(p_remember_pattern, ''))) >= 3 then
    insert into public.matter_aliases (organization_id, matter_id, kind, pattern, created_by)
    values (v_org, p_matter_id, p_remember_kind, btrim(p_remember_pattern), auth.uid())
    on conflict (organization_id, kind, lower(pattern)) do update set matter_id = excluded.matter_id;
  end if;

  return v_entry;
end;
$$;

-- Re-run matching on the caller's unassigned pending blocks (after adding a rule).
create or replace function public.tracker_rematch_my_pending()
returns integer
language plpgsql security definer set search_path = public
as $$
declare
  r record;
  m record;
  n int := 0;
begin
  if auth.uid() is null then
    raise exception 'not authenticated';
  end if;
  for r in
    select id, organization_id, app_name, window_title, document_path
    from public.activity_blocks
    where user_id = auth.uid() and status = 'pending' and matter_id is null
  loop
    m := null;
    select * into m from public.tracker_match_matter(r.organization_id, r.window_title, r.document_path) limit 1;
    update public.activity_blocks
    set suggested_matter_id = m.matter_id, match_score = m.score, match_reason = m.reason,
        suggested_billing_item_id = coalesce(suggested_billing_item_id,
          public.tracker_suggest_billing_item(r.organization_id, r.app_name, r.window_title, r.document_path))
    where id = r.id;
    n := n + 1;
  end loop;
  return n;
end;
$$;

-- ---------------------------------------------------------------------------
-- 8. Grants: matching + ingest are internal; only the RPCs users need are exposed
-- ---------------------------------------------------------------------------
revoke execute on function public.tracker_match_matter(uuid, text, text) from public, anon, authenticated;
revoke execute on function public.tracker_suggest_billing_item(uuid, text, text, text) from public, anon, authenticated;
revoke execute on function public.tracker_ingest_blocks(uuid, jsonb) from public, anon, authenticated;
grant execute on function public.tracker_match_matter(uuid, text, text) to service_role;
grant execute on function public.tracker_suggest_billing_item(uuid, text, text, text) to service_role;
grant execute on function public.tracker_ingest_blocks(uuid, jsonb) to service_role;

revoke execute on function public.create_tracker_pairing_code(uuid) from public, anon;
revoke execute on function public.revoke_tracker_device(uuid) from public, anon;
revoke execute on function public.approve_activity_blocks(uuid[], uuid, uuid, date, public.billing_basis, numeric, numeric, text, text, text) from public, anon;
revoke execute on function public.tracker_rematch_my_pending() from public, anon;
grant execute on function public.create_tracker_pairing_code(uuid) to authenticated;
grant execute on function public.revoke_tracker_device(uuid) to authenticated;
grant execute on function public.approve_activity_blocks(uuid[], uuid, uuid, date, public.billing_basis, numeric, numeric, text, text, text) to authenticated;
grant execute on function public.tracker_rematch_my_pending() to authenticated;
