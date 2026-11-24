import React from "react";
import type { SkillItem } from "../../types";
import * as ipc from "../../ipc";
import { CheckIcon, CloseIcon } from "../../icons";
import { Text } from "../ui/surfaces";

/*
 * Skills 多选：一枚输入框加一张下拉清单。
 *
 * 卡片与行的几何跟全站菜单同源（0.5px 发丝描边、4px 间距、8px 圆角、逐行浅灰底），
 * 不再自带一套输入框描边与投影。
 */

export const SkillsMultiSelect: React.FC<{
  value: string[];
  onChange: (value: string[]) => void;
  className?: string;
}> = ({ value, onChange, className = "" }) => {
  const [skills, setSkills] = React.useState<SkillItem[]>([]);
  const [query, setQuery] = React.useState("");
  const [open, setOpen] = React.useState(false);
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const [activeIndex, setActiveIndex] = React.useState(0);
  const container = React.useRef<HTMLDivElement>(null);
  const listId = React.useId();

  React.useEffect(() => {
    if (!open) return;
    const dismiss = (event: PointerEvent) => {
      if (!container.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", dismiss);
    return () => document.removeEventListener("pointerdown", dismiss);
  }, [open]);

  React.useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void ipc
      .skillsList()
      .then((items) => {
        if (!cancelled) setSkills(items);
      })
      .catch((failure) => {
        if (!cancelled) setError(failure instanceof Error ? failure.message : "读取 Skills 失败");
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const available = React.useMemo(() => {
    const needle = query.trim().toLowerCase();
    return skills.filter(
      (skill) =>
        needle.length === 0 ||
        skill.id.toLowerCase().includes(needle) ||
        skill.name.toLowerCase().includes(needle) ||
        skill.description.toLowerCase().includes(needle),
    );
  }, [query, skills]);

  const add = (id: string) => {
    onChange(value.includes(id) ? value.filter((item) => item !== id) : [...value, id]);
    setOpen(true);
  };
  const remove = (id: string) => onChange(value.filter((item) => item !== id));
  const labelFor = (id: string) => skills.find((skill) => skill.id === id)?.name || id;

  return (
    <div
      ref={container}
      className={`relative ${className}`}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          setOpen(false);
        }
      }}
    >
      <div
        className="min-h-8 px-2 py-1 rounded-sm bg-surface hairline border-border-input focus-within:border-focus flex flex-wrap items-center gap-1 transition-colors duration-100"
        onClick={() => setOpen(true)}
      >
        {value.map((id) => (
          <span
            key={id}
            className="inline-flex items-center gap-1 rounded-full bg-active px-2 py-0.5 text-xs text-text-primary"
          >
            {labelFor(id)}
            <button
              type="button"
              aria-label={`移除 ${labelFor(id)}`}
              className="text-text-tertiary hover:text-text-primary"
              onClick={(event) => {
                event.stopPropagation();
                remove(id);
              }}
            >
              <CloseIcon size={12} />
            </button>
          </span>
        ))}
        <input
          role="combobox"
          aria-label="搜索 Skills"
          aria-expanded={open}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={
            open && available[activeIndex] ? `${listId}-${activeIndex}` : undefined
          }
          value={query}
          onFocus={() => setOpen(true)}
          onChange={(event) => {
            setQuery(event.target.value);
            setActiveIndex(0);
            setOpen(true);
          }}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault();
              setOpen(true);
              setActiveIndex((index) =>
                Math.max(
                  0,
                  Math.min(available.length - 1, index + (event.key === "ArrowDown" ? 1 : -1)),
                ),
              );
            }
            if (event.key === "Enter" && open) {
              event.preventDefault();
              if (available[activeIndex]) add(available[activeIndex].id);
            }
            if (event.key === "Backspace" && query.length === 0 && value.length > 0) {
              remove(value[value.length - 1]);
            }
          }}
          placeholder={value.length === 0 ? "搜索并选择 Skills" : "继续搜索"}
          className="min-w-32 flex-1 h-6 bg-transparent outline-none text-row text-text-primary placeholder:text-text-placeholder"
        />
      </div>
      {open && (
        <div
          id={listId}
          role="listbox"
          aria-label="Skills"
          aria-multiselectable="true"
          className="absolute z-20 mt-1 w-full max-h-56 overflow-y-auto p-1 rounded-sm bg-surface hairline border-border-hairline shadow-menu"
        >
          {error !== null ? (
            <div className="px-2 py-2">
              <Text size="xs" tone="danger">
                {error}
              </Text>
            </div>
          ) : loading ? (
            <div className="px-2 py-2">
              <Text size="xs" tone="secondary">
                正在读取 Skills
              </Text>
            </div>
          ) : available.length === 0 ? (
            <div className="px-2 py-2">
              <Text size="xs" tone="secondary">
                没有匹配的 Skill
              </Text>
            </div>
          ) : (
            available.map((skill, index) => {
              const selected = value.includes(skill.id);
              return (
                <button
                  id={`${listId}-${index}`}
                  key={skill.id}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  className={`w-full h-8 flex items-center gap-2 px-2 rounded-sm text-left transition-colors duration-100 ${
                    selected || index === activeIndex ? "bg-hover" : ""
                  } hover:bg-hover`}
                  onMouseEnter={() => setActiveIndex(index)}
                  onClick={() => add(skill.id)}
                >
                  <span className="w-4 shrink-0 flex justify-center text-text-primary">
                    {selected && <CheckIcon size={13} />}
                  </span>
                  <span className="min-w-0 flex-1 flex items-baseline gap-2">
                    <Text size="row" truncate className="min-w-0 shrink">
                      {skill.name || skill.id}
                    </Text>
                    <Text size="xs" tone="tertiary" truncate className="min-w-0">
                      {skill.id}
                    </Text>
                  </span>
                </button>
              );
            })
          )}
        </div>
      )}
    </div>
  );
};
