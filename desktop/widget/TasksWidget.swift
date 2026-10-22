// TrusCo Tasks widget (macOS 14+, desktop or Notification Centre). View only: it shows the
// user's open tasks from the snapshot TrusCo Tracker writes to the shared App Group folder,
// works out urgency itself as time passes, and opens TrusCo Tracker on the Tasks tab when clicked.

import SwiftUI
import WidgetKit

let appGroup = "H7567N8H2N.app.trusco.tracker"

// MARK: - Data

struct TaskItem: Decodable, Identifiable {
    let id: String
    let title: String
    let project: String
    let due: Date?
    let dueHasTime: Bool
    let kind: String
    let priority: String
    let hours: Double
    let slot: Date?
}

struct Snapshot: Decodable {
    let generatedAt: Date
    let paired: Bool
    let tasks: [TaskItem]
}

enum SnapshotStore {
    static func load() -> Snapshot? {
        guard let dir = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup),
              let data = try? Data(contentsOf: dir.appendingPathComponent("tasks.json"))
        else { return nil }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        decoder.dateDecodingStrategy = .iso8601
        return try? decoder.decode(Snapshot.self, from: data)
    }
}

// MARK: - Urgency and labels (same rules as the app: overdue, due within 3 h, today, later)

enum Urgency {
    case overdue, soon, today, later, none

    var color: Color {
        switch self {
        case .overdue: return Color(red: 0.85, green: 0.18, blue: 0.13)
        case .soon, .today: return Color(red: 0.88, green: 0.53, blue: 0.0)
        case .later, .none: return Color.secondary
        }
    }
}

private let hourMinute: DateFormatter = {
    let f = DateFormatter()
    f.dateFormat = "HH:mm"
    return f
}()

private let weekday: DateFormatter = {
    let f = DateFormatter()
    f.dateFormat = "EEE"
    return f
}()

private let dayMonth: DateFormatter = {
    let f = DateFormatter()
    f.dateFormat = "d MMM"
    return f
}()

extension TaskItem {
    func urgency(at now: Date) -> Urgency {
        guard let due else { return .none }
        if due < now { return .overdue }
        if dueHasTime && due.timeIntervalSince(now) <= 3 * 3600 { return .soon }
        return Calendar.current.isDate(due, inSameDayAs: now) ? .today : .later
    }

    func dueLabel(at now: Date) -> String {
        guard let due else { return "No due date" }
        let cal = Calendar.current
        let days = cal.dateComponents([.day], from: cal.startOfDay(for: now), to: cal.startOfDay(for: due)).day ?? 0
        let day: String
        switch days {
        case 0: day = "Today"
        case 1: day = "Tomorrow"
        case -1: day = "Yesterday"
        case 2...6, -6 ... -2: day = weekday.string(from: due)
        default: day = dayMonth.string(from: due)
        }
        let time = dueHasTime ? " " + hourMinute.string(from: due) : ""
        switch urgency(at: now) {
        case .overdue:
            let relative = ["Today", "Tomorrow", "Yesterday"].contains(day) ? day.lowercased() : day
            return "Overdue · \(relative)\(time)"
        case .soon:
            let minutes = max(0, Int(due.timeIntervalSince(now) / 60))
            let left = minutes >= 60 ? "\(minutes / 60)h\(minutes % 60 > 0 ? " \(minutes % 60)m" : "")" : "\(minutes)m"
            return "\(day)\(time) · in \(left)"
        default:
            return day + time
        }
    }
}

// MARK: - Timeline

struct Entry: TimelineEntry {
    let date: Date
    let snapshot: Snapshot?
}

struct Provider: TimelineProvider {
    func placeholder(in context: Context) -> Entry {
        Entry(date: .now, snapshot: .sample)
    }

    func getSnapshot(in context: Context, completion: @escaping (Entry) -> Void) {
        completion(Entry(date: .now, snapshot: context.isPreview ? .sample : (SnapshotStore.load() ?? .sample)))
    }

    /// One entry now, plus one at every moment a colour or label changes over the next 12 hours
    /// (3 h before and at each due time) and every 30 minutes for "in 45m" style labels.
    func getTimeline(in context: Context, completion: @escaping (Timeline<Entry>) -> Void) {
        let snapshot = SnapshotStore.load()
        let now = Date()
        var dates: Set<Date> = [now]
        for task in snapshot?.tasks ?? [] {
            guard let due = task.due else { continue }
            for moment in [due.addingTimeInterval(-3 * 3600), due] where moment > now && moment < now.addingTimeInterval(12 * 3600) {
                dates.insert(moment)
            }
        }
        for step in 1...24 {
            dates.insert(now.addingTimeInterval(Double(step) * 1800))
        }
        let entries = dates.sorted().map { Entry(date: $0, snapshot: snapshot) }
        // The app also asks for a reload whenever tasks change.
        completion(Timeline(entries: entries, policy: .after(now.addingTimeInterval(30 * 60))))
    }
}

// MARK: - Views

struct TaskRow: View {
    let task: TaskItem
    let now: Date
    let showProject: Bool

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Circle()
                .fill(task.urgency(at: now).color)
                .frame(width: 7, height: 7)
                .alignmentGuide(.firstTextBaseline) { $0[.bottom] - 1 }
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 3) {
                    if task.priority == "high" {
                        Text("!").fontWeight(.heavy).foregroundStyle(Urgency.overdue.color)
                    }
                    Text(task.title).fontWeight(.semibold).lineLimit(1)
                }
                .font(.system(size: 12))
                HStack(spacing: 4) {
                    Text(task.dueLabel(at: now))
                        .foregroundStyle(task.urgency(at: now) == .later || task.urgency(at: now) == .none ? Color.secondary : task.urgency(at: now).color)
                    if task.kind == "time" && task.hours > 0 {
                        Text("· ⏱ \(Self.hours(task.hours))").foregroundStyle(.secondary)
                    }
                    if showProject {
                        Text("· \(task.project)").foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                .font(.system(size: 10))
                .lineLimit(1)
            }
        }
    }

    static func hours(_ h: Double) -> String {
        let minutes = Int((h * 60).rounded())
        if minutes < 60 { return "\(minutes)m" }
        return minutes % 60 == 0 ? "\(minutes / 60)h" : "\(minutes / 60)h \(minutes % 60)m"
    }
}

struct TasksWidgetView: View {
    let entry: Entry
    /// Lets the image preview pick a size; WidgetKit supplies it through the environment.
    var familyOverride: WidgetFamily? = nil
    @Environment(\.widgetFamily) private var environmentFamily

    private var family: WidgetFamily { familyOverride ?? environmentFamily }

    private var rows: Int {
        switch family {
        case .systemSmall, .systemMedium: return 3
        default: return 8
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: family == .systemSmall ? 6 : 7) {
            header
            if let snapshot = entry.snapshot, snapshot.paired {
                if snapshot.tasks.isEmpty {
                    Spacer()
                    Text("No open tasks").font(.system(size: 12)).foregroundStyle(.secondary)
                    Spacer()
                } else {
                    ForEach(snapshot.tasks.prefix(rows)) { task in
                        TaskRow(task: task, now: entry.date, showProject: family == .systemLarge)
                    }
                    if snapshot.tasks.count > rows && family != .systemSmall {
                        Text("+ \(snapshot.tasks.count - rows) more").font(.system(size: 10)).foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 0)
                }
            } else {
                Spacer()
                Text("Open TrusCo Tracker and connect it to TrusCo to see your tasks.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                Spacer()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .widgetURL(URL(string: "trusco-tracker://tasks"))
    }

    private var header: some View {
        let tasks = entry.snapshot?.tasks ?? []
        let overdue = tasks.filter { $0.urgency(at: entry.date) == .overdue }.count
        let dueToday = tasks.filter { [.soon, .today].contains($0.urgency(at: entry.date)) }.count
        return HStack(spacing: 6) {
            Image(systemName: "checklist").foregroundStyle(Color(red: 0.18, green: 0.42, blue: 0.37))
            Text("Tasks").font(.system(size: 13, weight: .bold))
            Spacer(minLength: 4)
            if overdue > 0 { CountBadge(count: overdue, color: Urgency.overdue.color) }
            if dueToday > 0 { CountBadge(count: dueToday, color: Urgency.today.color) }
        }
    }
}

struct CountBadge: View {
    let count: Int
    let color: Color

    var body: some View {
        Text("\(count)")
            .font(.system(size: 10, weight: .bold))
            .foregroundStyle(.white)
            .padding(.horizontal, 6)
            .padding(.vertical, 1)
            .background(Capsule().fill(color))
    }
}

// MARK: - Widget

struct TasksWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "TrusCoTasks", provider: Provider()) { entry in
            TasksWidgetView(entry: entry)
                .containerBackground(.background, for: .widget)
        }
        .configurationDisplayName("TrusCo Tasks")
        .description("Your TrusCo tasks with due dates and urgency.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}

#if !WIDGET_PREVIEW
@main
struct TrusCoWidgets: WidgetBundle {
    var body: some Widget {
        TasksWidget()
    }
}
#endif

// MARK: - Sample data (widget gallery placeholder)

extension Snapshot {
    static var sample: Snapshot {
        let now = Date()
        func at(_ hours: Double) -> Date { now.addingTimeInterval(hours * 3600) }
        return Snapshot(generatedAt: now, paired: true, tasks: [
            TaskItem(id: "1", title: "Call client about the settlement offer", project: "Smit divorce", due: at(-2), dueHasTime: true, kind: "todo", priority: "high", hours: 0, slot: nil),
            TaskItem(id: "2", title: "File founding affidavit", project: "Smit divorce", due: at(2), dueHasTime: true, kind: "todo", priority: "normal", hours: 0, slot: nil),
            TaskItem(id: "3", title: "Draft parenting plan", project: "Smit divorce", due: at(26), dueHasTime: false, kind: "time", priority: "normal", hours: 2.5, slot: at(21)),
            TaskItem(id: "4", title: "Send fee note", project: "Botha estate", due: at(96), dueHasTime: false, kind: "todo", priority: "normal", hours: 0, slot: nil),
        ])
    }
}
