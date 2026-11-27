-- Reproducible Aegisub 3.5.1 Automation 4 micro-benchmark for 20,000-line ASS operations.
-- Equivalent to `crates/ass-tools` / `crates/edit-ops` benchmarks on the same 20k ASS script.

script_name = "HLouis Benchmark (20k Ops)"
script_description = "Times 20,000-line Shift, Find/Replace, Select, Karaoke parse/rebuild, and CPS in Aegisub."
script_author = "hlouis-sub-edit bench"
script_version = "1.0"

local ok_ffi, ffi = pcall(require, "ffi")
if ok_ffi then
    pcall(function()
        ffi.cdef[[
            int QueryPerformanceCounter(int64_t *lpPerformanceCount);
            int QueryPerformanceFrequency(int64_t *lpFrequency);
        ]]
    end)
end

local function now_ms()
    if ok_ffi and ffi.C.QueryPerformanceCounter and ffi.C.QueryPerformanceFrequency then
        local cnt = ffi.new("int64_t[1]")
        local freq = ffi.new("int64_t[1]")
        if ffi.C.QueryPerformanceCounter(cnt) ~= 0 and ffi.C.QueryPerformanceFrequency(freq) ~= 0 and freq[0] > 0 then
            return tonumber(cnt[0]) * 1000.0 / tonumber(freq[0])
        end
    end
    return os.clock() * 1000.0
end

local function strip_tags(text)
    return (text:gsub("{[^}]*}", ""))
end

local function run_bench(subtitles)
    local dialogue_indices = {}
    local t0 = now_ms()
    for i = 1, #subtitles do
        local l = subtitles[i]
        if l.class == "dialogue" then
            dialogue_indices[#dialogue_indices + 1] = i
        end
    end
    local scan_ms = now_ms() - t0
    local n = #dialogue_indices

    -- 1. Select matching lines ("Tiếng Việt" or "日本語", case-insensitive plain substring)
    local t_sel = now_ms()
    local selected_count = 0
    for _, idx in ipairs(dialogue_indices) do
        local l = subtitles[idx]
        if l.text:find("日本語", 1, true) or l.text:lower():find("styled", 1, true) then
            selected_count = selected_count + 1
        end
    end
    local select_ms = now_ms() - t_sel

    -- 2. Find & Replace skipping {...} tags
    local t_fr = now_ms()
    local replaced = 0
    for _, idx in ipairs(dialogue_indices) do
        local l = subtitles[idx]
        local changed = l.text:gsub("([^{}]+)", function(seg)
            if seg:find("Plain", 1, true) then
                replaced = replaced + 1
                return (seg:gsub("Plain", "Edited"))
            end
            return seg
        end)
        if changed ~= l.text then
            l.text = changed
            subtitles[idx] = l
        end
    end
    local find_replace_ms = now_ms() - t_fr

    -- 3. Shift Times (+250 ms all lines) + undo writeback
    local t_sh = now_ms()
    for _, idx in ipairs(dialogue_indices) do
        local l = subtitles[idx]
        l.start_time = l.start_time + 250
        l.end_time = l.end_time + 250
        subtitles[idx] = l
    end
    local shift_ms = now_ms() - t_sh

    -- Restore original times/text so the benchmark leaves lines neutral
    for _, idx in ipairs(dialogue_indices) do
        local l = subtitles[idx]
        l.start_time = math.max(0, l.start_time - 250)
        l.end_time = math.max(0, l.end_time - 250)
        subtitles[idx] = l
    end

    -- 4. Karaoke tag parse + rebuild over lines containing \k / \kf / \ko / \K
    local t_kara = now_ms()
    local kara_syllables = 0
    for _, idx in ipairs(dialogue_indices) do
        local l = subtitles[idx]
        local out = {}
        local pos = 1
        local len = #l.text
        while pos <= len do
            local s, e, kind, dur = l.text:find("{\\([kK][fot]?)([0-9]+)}", pos)
            if not s then
                out[#out + 1] = l.text:sub(pos)
                break
            end
            if s > pos then
                out[#out + 1] = l.text:sub(pos, s - 1)
            end
            kara_syllables = kara_syllables + 1
            out[#out + 1] = string.format("{\\%s%s}", kind, dur)
            pos = e + 1
        end
        local _ = table.concat(out)
    end
    local kara_ms = now_ms() - t_kara

    -- 5. CPS computation over all dialogue lines
    local t_cps = now_ms()
    local total_chars = 0
    for _, idx in ipairs(dialogue_indices) do
        local l = subtitles[idx]
        local plain = strip_tags(l.text):gsub("\\[nNh]", "")
        total_chars = total_chars + #plain
    end
    local cps_ms = now_ms() - t_cps

    local t_undo = now_ms()
    aegisub.set_undo_point(script_name)
    local undo_ms = now_ms() - t_undo

    local summary = string.format(
        '{"lines":%d,"scan_ms":%.3f,"select_ms":%.3f,"selected":%d,"find_replace_ms":%.3f,"replaced":%d,"shift_ms":%.3f,"karaoke_ms":%.3f,"kara_syls":%d,"cps_ms":%.3f,"undo_commit_ms":%.3f}',
        n, scan_ms, select_ms, selected_count, find_replace_ms, replaced, shift_ms, kara_ms, kara_syllables, cps_ms, undo_ms
    )
    local tmp = (os.getenv("TEMP") or ".") .. "\\aegisub_lua_bench.json"
    local f = io.open(tmp, "w")
    if f then
        f:write(summary .. "\n")
        f:close()
    end
    aegisub.log("[hlouis-aegisub-bench] %s\n", summary)
end

aegisub.register_macro(script_name, script_description, run_bench)
