on run argv
  set mode to item 1 of argv
  set wanted to item 2 of argv
  tell application "Terminal"
    set matches to {}
    repeat with w in windows
      if mode is "tty" then
        repeat with t in tabs of w
          if tty of t is wanted then set end of matches to {w, t}
        end repeat
      else if (mode is "window" and (id of w as text) is wanted) or (mode is "title" and name of w is wanted) then
        set end of matches to {w}
      end if
    end repeat
    if (count of matches) is 1 then
      set matched to item 1 of matches
      set w to item 1 of matched
      if (count of matched) is 2 then set selected tab of w to item 2 of matched
      set miniaturized of w to false
      set index of w to 1
      activate
      return "focused"
    end if
  end tell
  return "missing"
end run
