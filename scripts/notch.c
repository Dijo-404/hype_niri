#include "ext-background-effect-v1-client-protocol.h"
#include <gdk/gdkwayland.h>
#include <gio/gdesktopappinfo.h>
#include <glib-unix.h>
#include <gtk-layer-shell.h>
#include <gtk/gtk.h>
#include <playerctl/playerctl.h>
#include <signal.h>
#include <string.h>

#define NOTCH_WIDTH 400
#define NOTCH_HEIGHT 820
#define MOTION_MS 150
#define WEATHER_URL "https://wttr.in/?format=%C|%t|%f|%h|%l"

typedef struct {
  gboolean on_break;
  guint remaining;
  gint64 deadline;
} FocusTimer;

typedef struct {
  gboolean running;
  gint64 started, elapsed;
} Stopwatch;

typedef enum { NOTCH_HIDDEN, NOTCH_PEEK, NOTCH_OPEN } NotchState;

typedef struct {
  GtkWidget *row, *scale, *value;
  const char *read[4], *set[7];
  int (*parse)(const char *);
  GCancellable *cancel;
  gboolean busy, dirty, syncing;
} Level;

typedef struct {
  GtkApplication *app;
  const char *scripts;
  FocusTimer timer;
  Stopwatch stopwatch;
  GPtrArray *views;
} NotchApp;

typedef struct {
  NotchApp *shared;
  GdkMonitor *monitor;
  GtkWidget *window, *morph, *surface, *header, *body, *summary;
  GtkWidget *media_card, *title, *artist, *previous, *play, *next;
  GtkWidget *media_icon, *header_wave, *card_wave;
  GtkWidget *phase, *countdown, *compact_timer, *timer_button, *ring, *message;
  GtkWidget *watch, *watch_state, *watch_button, *watch_ring;
  GtkWidget *weather_icon, *weather_temp, *forecast_icon, *forecast_temp,
      *forecast_condition, *forecast_detail, *forecast_place;
  GtkWidget *search, *results;
  GtkWidget *flow, *flow_button, *flow_meter, *flow_label, *flow_card,
      *flow_status, *flow_toggle, *flow_cancel;
  GDBusActionGroup *flow_actions;
  guint flow_watch;
  gboolean flow_visible, flow_recording;
  double flow_levels[64];
  PlayerctlPlayerManager *manager;
  PlayerctlPlayer *player;
  struct ext_background_effect_surface_v1 *blur;
  GCancellable *cancel;
  Level volume, brightness;
  guint close_source, open_source, timer_source, media_source, watch_source,
      weather_source;
  NotchState state;
  gboolean pinned;
} Notch;

static GtkWidget *revealer(GtkRevealerTransitionType type, gboolean shown);

static guint timer_duration(const FocusTimer *timer) {
  return timer->on_break ? 300 : 1500;
}

static const char *timer_phase(const FocusTimer *timer) {
  return timer->on_break ? "Break" : "Focus";
}

static const char *timer_update(FocusTimer *timer, gint64 now) {
  if (!timer->deadline)
    return NULL;
  gint64 delta = MAX((gint64)0, timer->deadline - now);
  timer->remaining = (guint)((delta + G_USEC_PER_SEC - 1) / G_USEC_PER_SEC);
  if (timer->remaining)
    return NULL;
  const char *completed = timer_phase(timer);
  timer->on_break = !timer->on_break;
  timer->remaining = timer_duration(timer);
  timer->deadline = 0;
  return completed;
}

static const char *timer_toggle(FocusTimer *timer, gint64 now) {
  const char *completed = timer_update(timer, now);
  timer->deadline =
      timer->deadline ? 0 : now + (gint64)timer->remaining * G_USEC_PER_SEC;
  return completed;
}

static void timer_skip(FocusTimer *timer, gint64 now) {
  timer->on_break = !timer->on_break;
  timer->remaining = timer_duration(timer);
  if (timer->deadline)
    timer->deadline = now + (gint64)timer->remaining * G_USEC_PER_SEC;
}

static gint64 stopwatch_elapsed(const Stopwatch *watch, gint64 now) {
  return watch->elapsed + (watch->running ? now - watch->started : 0);
}

static void stopwatch_toggle(Stopwatch *watch, gint64 now) {
  watch->elapsed = stopwatch_elapsed(watch, now);
  watch->started = now;
  watch->running = !watch->running;
}

static char *stopwatch_text(gint64 elapsed) {
  gint64 tenths = elapsed / 100000, seconds = tenths / 10;
  if (seconds >= 3600)
    return g_strdup_printf("%" G_GINT64_FORMAT ":%02d:%02d", seconds / 3600,
                           (int)(seconds / 60 % 60), (int)(seconds % 60));
  return g_strdup_printf("%02d:%02d.%d", (int)(seconds / 60),
                         (int)(seconds % 60), (int)(tenths % 10));
}

static const char *weather_icon(const char *condition) {
  static const char *const map[][2] = {
      {"thunder", "weather-storm-symbolic"},
      {"snow", "weather-snow-symbolic"},
      {"sleet", "weather-snow-symbolic"},
      {"ice", "weather-snow-symbolic"},
      {"blizzard", "weather-snow-symbolic"},
      {"shower", "weather-showers-scattered-symbolic"},
      {"drizzle", "weather-showers-scattered-symbolic"},
      {"rain", "weather-showers-symbolic"},
      {"fog", "weather-fog-symbolic"},
      {"mist", "weather-fog-symbolic"},
      {"haze", "weather-fog-symbolic"},
      {"smoke", "weather-fog-symbolic"},
      {"partly", "weather-few-clouds-symbolic"},
      {"cloud", "weather-overcast-symbolic"},
      {"overcast", "weather-overcast-symbolic"},
      {"clear", "weather-clear-symbolic"},
      {"sunny", "weather-clear-symbolic"},
  };
  g_autofree char *lower = g_ascii_strdown(condition, -1);
  for (gsize i = 0; i < G_N_ELEMENTS(map); i++)
    if (strstr(lower, map[i][0]))
      return map[i][1];
  return "weather-overcast-symbolic";
}

static int parse_volume(const char *text) {
  const char *start = text ? strstr(text, "Volume:") : NULL;
  if (!start)
    return -1;
  char *end;
  double value = g_ascii_strtod(start + 7, &end);
  if (end == start + 7)
    return -1;
  return CLAMP((int)(value * 100 + 0.5), 0, 100);
}

static int parse_brightness(const char *text) {
  g_auto(GStrv) field = g_strsplit(text ? text : "", ",", 6);
  if (g_strv_length(field) < 5)
    return -1;
  char *end;
  gint64 percent = g_ascii_strtoll(field[3], &end, 10);
  if (end == field[3] || *end != '%')
    return -1;
  return CLAMP((int)percent, 0, 100);
}

static void add_class(GtkWidget *widget, const char *style) {
  gtk_style_context_add_class(gtk_widget_get_style_context(widget), style);
}

static void set_class(GtkWidget *widget, const char *style, gboolean on) {
  GtkStyleContext *context = gtk_widget_get_style_context(widget);
  if (on)
    gtk_style_context_add_class(context, style);
  else
    gtk_style_context_remove_class(context, style);
}

static GtkWidget *text_label(const char *text, const char *style) {
  GtkWidget *label = gtk_label_new(text);
  gtk_label_set_xalign(GTK_LABEL(label), 0);
  gtk_label_set_ellipsize(GTK_LABEL(label), PANGO_ELLIPSIZE_END);
  if (style)
    add_class(label, style);
  return label;
}

static void set_button_icon(GtkWidget *button, const char *icon,
                            const char *name) {
  gboolean large = gtk_style_context_has_class(
      gtk_widget_get_style_context(button), "large");
  gtk_button_set_image(
      GTK_BUTTON(button),
      gtk_image_new_from_icon_name(icon, large ? GTK_ICON_SIZE_LARGE_TOOLBAR
                                               : GTK_ICON_SIZE_BUTTON));
  gtk_widget_set_tooltip_text(button, name);
  atk_object_set_name(gtk_widget_get_accessible(button), name);
}

static GtkWidget *icon_button(const char *icon, const char *name,
                              GCallback callback, Notch *n) {
  GtkWidget *button = gtk_button_new();
  gtk_widget_set_focus_on_click(button, FALSE);
  set_button_icon(button, icon, name);
  gtk_widget_set_valign(button, GTK_ALIGN_CENTER);
  g_signal_connect(button, "clicked", callback, n);
  return button;
}

static void pack(GtkWidget *box, GtkWidget *child, gboolean expand) {
  gtk_box_pack_start(GTK_BOX(box), child, expand, expand, 0);
}

static GtkWidget *card(const char *name) {
  GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
  gtk_widget_set_name(box, name);
  add_class(box, "card");
  return box;
}

static void show_message(Notch *n, const char *text) {
  gtk_label_set_text(GTK_LABEL(n->message), text);
  gtk_widget_set_visible(n->message, *text != '\0');
}

static void refresh_compact(Notch *n) {
  gboolean focus = n->shared->timer.deadline || n->shared->timer.on_break ||
                   n->shared->timer.remaining != 1500;
  gint64 elapsed =
      stopwatch_elapsed(&n->shared->stopwatch, g_get_monotonic_time());
  g_autofree char *watch = stopwatch_text(elapsed);
  gtk_label_set_text(GTK_LABEL(n->compact_timer),
                     focus ? gtk_label_get_text(GTK_LABEL(n->countdown))
                           : watch);
  gtk_widget_set_visible(n->compact_timer, focus || elapsed);
}

static void refresh_timer(Notch *n) {
  g_autofree char *text =
      g_strdup_printf("%02u:%02u", n->shared->timer.remaining / 60,
                      n->shared->timer.remaining % 60);
  FocusTimer *timer = &n->shared->timer;
  gtk_label_set_text(GTK_LABEL(n->phase), timer->on_break ? "Break" : "Focus");
  gtk_label_set_text(GTK_LABEL(n->countdown), text);
  set_class(n->phase, "break", timer->on_break);
  set_class(n->ring, "break", timer->on_break);
  gtk_widget_queue_draw(n->ring);
  g_autofree char *name = g_strdup_printf(
      "%s %s timer", n->shared->timer.deadline ? "Pause" : "Start",
      n->shared->timer.on_break ? "break" : "focus");
  set_button_icon(n->timer_button,
                  n->shared->timer.deadline ? "media-playback-pause-symbolic"
                                            : "media-playback-start-symbolic",
                  name);
  refresh_compact(n);
}

static void refresh_stopwatch(Notch *n) {
  Stopwatch *watch = &n->shared->stopwatch;
  g_autofree char *text =
      stopwatch_text(stopwatch_elapsed(watch, g_get_monotonic_time()));
  gtk_label_set_text(GTK_LABEL(n->watch), text);
  gtk_widget_queue_draw(n->watch_ring);
  gtk_label_set_text(GTK_LABEL(n->watch_state), watch->running   ? "Running"
                                                : watch->elapsed ? "Paused"
                                                                 : "Stopwatch");
  set_button_icon(n->watch_button,
                  watch->running ? "media-playback-pause-symbolic"
                                 : "media-playback-start-symbolic",
                  watch->running ? "Pause stopwatch" : "Start stopwatch");
  refresh_compact(n);
}

static void notify_completion(Notch *n, const char *phase) {
  if (!phase)
    return;
  g_autofree char *title = g_strdup_printf("%s complete", phase);
  g_autoptr(GNotification) notification = g_notification_new(title);
  g_notification_set_body(notification,
                          !strcmp(phase, "Focus")
                              ? "Take a five-minute break."
                              : "Ready for another focus session.");
  g_application_send_notification(G_APPLICATION(n->shared->app), "focus-timer",
                                  notification);
}

static gboolean timer_tick(gpointer data) {
  Notch *n = data;
  notify_completion(n, timer_update(&n->shared->timer, g_get_monotonic_time()));
  refresh_timer(n);
  if (!n->shared->timer.deadline) {
    n->timer_source = 0;
    return G_SOURCE_REMOVE;
  }
  return G_SOURCE_CONTINUE;
}

static gboolean watch_tick(gpointer data);

static void sync_clocks(NotchApp *a) {
  for (guint i = 0; i < a->views->len; i++) {
    Notch *v = g_ptr_array_index(a->views, i);
    g_clear_handle_id(&v->timer_source, g_source_remove);
    g_clear_handle_id(&v->watch_source, g_source_remove);
    if (a->timer.deadline)
      v->timer_source = g_timeout_add_seconds(1, timer_tick, v);
    if (a->stopwatch.running)
      v->watch_source = g_timeout_add(100, watch_tick, v);
    refresh_timer(v);
    refresh_stopwatch(v);
  }
}

static void skip_clicked(GtkButton *button G_GNUC_UNUSED, gpointer data) {
  Notch *n = data;
  timer_skip(&n->shared->timer, g_get_monotonic_time());
  sync_clocks(n->shared);
}

static void timer_clicked(GtkButton *button G_GNUC_UNUSED, gpointer data) {
  Notch *n = data;
  notify_completion(n, timer_toggle(&n->shared->timer, g_get_monotonic_time()));
  sync_clocks(n->shared);
}

static void reset_clicked(GtkButton *button G_GNUC_UNUSED, gpointer data) {
  Notch *n = data;
  n->shared->timer = (FocusTimer){.remaining = 1500};
  sync_clocks(n->shared);
}

static gboolean watch_tick(gpointer data) {
  refresh_stopwatch(data);
  return G_SOURCE_CONTINUE;
}

static void watch_clicked(GtkButton *button G_GNUC_UNUSED, gpointer data) {
  Notch *n = data;
  stopwatch_toggle(&n->shared->stopwatch, g_get_monotonic_time());
  sync_clocks(n->shared);
}

static void watch_reset_clicked(GtkButton *button G_GNUC_UNUSED,
                                gpointer data) {
  Notch *n = data;
  n->shared->stopwatch = (Stopwatch){0};
  sync_clocks(n->shared);
}

static void apply_weather(Notch *n, const char *text) {
  g_autofree char *clean = g_strstrip(g_strdup(text ? text : ""));
  g_auto(GStrv) field = g_strsplit(clean, "|", 5);
  gboolean ok = g_strv_length(field) == 5 && *field[0] && *field[1];
  const char *icon = ok ? weather_icon(field[0]) : "weather-overcast-symbolic";
  const char *temp = ok ? field[1] + (field[1][0] == '+') : "--";
  g_autofree char *detail =
      ok ? g_strdup_printf("Feels %s · %s humidity",
                           field[2] + (field[2][0] == '+'), field[3])
         : g_strdup("Retrying in 30 minutes");
  gtk_image_set_from_icon_name(GTK_IMAGE(n->weather_icon), icon,
                               GTK_ICON_SIZE_MENU);
  gtk_image_set_from_icon_name(GTK_IMAGE(n->forecast_icon), icon,
                               GTK_ICON_SIZE_DND);
  gtk_label_set_text(GTK_LABEL(n->weather_temp), temp);
  gtk_label_set_text(GTK_LABEL(n->forecast_temp), temp);
  gtk_label_set_text(GTK_LABEL(n->forecast_condition),
                     ok ? field[0] : "Weather unavailable");
  gtk_label_set_text(GTK_LABEL(n->forecast_detail), detail);
  // City only; region and country crowd out the detail line.
  g_auto(GStrv) place = g_strsplit(ok ? field[4] : "", ",", 2);
  gtk_label_set_text(GTK_LABEL(n->forecast_place), place[0] ? place[0] : "");
}

static void weather_done(GObject *source, GAsyncResult *result, gpointer data) {
  g_autofree char *out = NULL;
  g_autoptr(GError) error = NULL;
  gboolean ok = g_subprocess_communicate_utf8_finish(
      G_SUBPROCESS(source), result, &out, NULL, &error);
  if (g_error_matches(error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
    return;
  apply_weather(data, ok && g_subprocess_get_successful(G_SUBPROCESS(source))
                          ? out
                          : NULL);
}

static gboolean run_async(const char *const *argv, GCancellable *cancel,
                          GAsyncReadyCallback done, gpointer data) {
  g_autoptr(GSubprocess) process = g_subprocess_newv(
      argv, G_SUBPROCESS_FLAGS_STDOUT_PIPE | G_SUBPROCESS_FLAGS_STDERR_SILENCE,
      NULL);
  if (process)
    g_subprocess_communicate_utf8_async(process, NULL, cancel, done, data);
  return process != NULL;
}

static gboolean fetch_weather(gpointer data) {
  Notch *n = data;
  static const char *const argv[] = {"curl", "-fsS",      "--max-time",
                                     "15",   WEATHER_URL, NULL};
  if (!run_async(argv, n->cancel, weather_done, n))
    apply_weather(n, NULL);
  return G_SOURCE_CONTINUE;
}

static void level_read_done(GObject *source, GAsyncResult *result,
                            gpointer data) {
  Level *level = data;
  g_autofree char *out = NULL;
  g_autoptr(GError) error = NULL;
  gboolean ok = g_subprocess_communicate_utf8_finish(
      G_SUBPROCESS(source), result, &out, NULL, &error);
  if (g_error_matches(error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
    return;
  int percent = ok && g_subprocess_get_successful(G_SUBPROCESS(source))
                    ? level->parse(out)
                    : -1;
  gtk_widget_set_visible(level->row, percent >= 0);
  if (percent < 0 || level->busy)
    return;
  level->syncing = TRUE;
  gtk_range_set_value(GTK_RANGE(level->scale), percent);
  level->syncing = FALSE;
}

static void level_read(Level *level) {
  if (!run_async(level->read, level->cancel, level_read_done, level))
    gtk_widget_set_visible(level->row, FALSE);
}

static void level_apply(Level *level);

static void level_applied(GObject *source, GAsyncResult *result,
                          gpointer data) {
  Level *level = data;
  g_autoptr(GError) error = NULL;
  g_subprocess_wait_finish(G_SUBPROCESS(source), result, &error);
  if (g_error_matches(error, G_IO_ERROR, G_IO_ERROR_CANCELLED))
    return;
  level->busy = FALSE;
  if (level->dirty)
    level_apply(level);
}

static void level_apply(Level *level) {
  g_autofree char *value = g_strdup_printf(
      "%d%%", (int)gtk_range_get_value(GTK_RANGE(level->scale)));
  const char *argv[G_N_ELEMENTS(level->set) + 1] = {0};
  gsize i = 0;
  for (; level->set[i]; i++)
    argv[i] = level->set[i];
  argv[i] = value;
  g_autoptr(GSubprocess) process = g_subprocess_newv(
      argv,
      G_SUBPROCESS_FLAGS_STDOUT_SILENCE | G_SUBPROCESS_FLAGS_STDERR_SILENCE,
      NULL);
  level->dirty = FALSE;
  level->busy = process != NULL;
  if (process)
    g_subprocess_wait_async(process, level->cancel, level_applied, level);
}

static void level_changed(GtkRange *range, gpointer data) {
  Level *level = data;
  g_autofree char *text =
      g_strdup_printf("%d%%", (int)gtk_range_get_value(range));
  gtk_label_set_text(GTK_LABEL(level->value), text);
  if (level->syncing)
    return;
  if (level->busy)
    level->dirty = TRUE;
  else
    level_apply(level);
}

static void add_result(Notch *n, GAppInfo *app, const char *text, GIcon *icon) {
  GtkWidget *row = gtk_list_box_row_new();
  if (app)
    g_object_set_data_full(G_OBJECT(row), "app", app, g_object_unref);
  GtkWidget *box = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
  GtkWidget *image =
      icon ? gtk_image_new_from_gicon(icon, GTK_ICON_SIZE_LARGE_TOOLBAR)
           : gtk_image_new_from_icon_name("web-browser-symbolic",
                                          GTK_ICON_SIZE_LARGE_TOOLBAR);
  gtk_image_set_pixel_size(GTK_IMAGE(image), 22);
  pack(box, image, FALSE);
  pack(box, text_label(text, NULL), TRUE);
  gtk_container_add(GTK_CONTAINER(row), box);
  gtk_widget_show_all(row);
  gtk_list_box_insert(GTK_LIST_BOX(n->results), row, -1);
}

static void search_changed(GtkEditable *entry, gpointer data) {
  Notch *n = data;
  g_autoptr(GList) rows = gtk_container_get_children(GTK_CONTAINER(n->results));
  for (GList *row = rows; row; row = row->next)
    gtk_widget_destroy(row->data);
  const char *query = gtk_entry_get_text(GTK_ENTRY(entry));
  gtk_widget_set_visible(n->results, *query != '\0');
  if (!*query)
    return;
  gchar ***groups = g_desktop_app_info_search(query);
  guint count = 0;
  for (gchar ***group = groups; *group; group++) {
    for (gchar **id = *group; *id && count < 5; id++) {
      GDesktopAppInfo *app = g_desktop_app_info_new(*id);
      if (app && g_app_info_should_show(G_APP_INFO(app))) {
        add_result(n, G_APP_INFO(app), g_app_info_get_name(G_APP_INFO(app)),
                   g_app_info_get_icon(G_APP_INFO(app)));
        count++;
      } else if (app)
        g_object_unref(app);
    }
    g_strfreev(*group);
  }
  g_free(groups);
  gboolean url =
      g_str_has_prefix(query, "https://") || g_str_has_prefix(query, "http://");
  g_autofree char *web =
      g_strdup_printf(url ? "Open %s" : "Search the web for “%s”", query);
  add_result(n, NULL, web, NULL);
  gtk_list_box_select_row(
      GTK_LIST_BOX(n->results),
      gtk_list_box_get_row_at_index(GTK_LIST_BOX(n->results), 0));
}

static void collapse_clicked(GtkButton *button, gpointer data);

static void launch_row(GtkListBox *list G_GNUC_UNUSED, GtkListBoxRow *row,
                       gpointer data) {
  Notch *n = data;
  GAppInfo *app = g_object_get_data(G_OBJECT(row), "app");
  const char *query = gtk_entry_get_text(GTK_ENTRY(n->search));
  g_autoptr(GError) error = NULL;
  g_autoptr(GdkAppLaunchContext) context =
      gdk_display_get_app_launch_context(gtk_widget_get_display(n->window));
  gboolean ok;
  if (app)
    ok = g_app_info_launch(app, NULL, G_APP_LAUNCH_CONTEXT(context), &error);
  else {
    g_autofree char *escaped = g_uri_escape_string(query, NULL, FALSE);
    g_autofree char *uri =
        g_str_has_prefix(query, "https://") ||
                g_str_has_prefix(query, "http://")
            ? g_strdup(query)
            : g_strconcat("https://duckduckgo.com/?q=", escaped, NULL);
    ok = g_app_info_launch_default_for_uri(uri, G_APP_LAUNCH_CONTEXT(context),
                                           &error);
  }
  if (!ok) {
    show_message(n, error->message);
    return;
  }
  collapse_clicked(NULL, n);
}

static void search_activate(GtkEntry *entry G_GNUC_UNUSED, gpointer data) {
  Notch *n = data;
  GtkListBoxRow *row = gtk_list_box_get_selected_row(GTK_LIST_BOX(n->results));
  if (row)
    launch_row(NULL, row, n);
}

static gboolean search_key(GtkWidget *entry G_GNUC_UNUSED, GdkEventKey *event,
                           gpointer data) {
  Notch *n = data;
  int step = event->keyval == GDK_KEY_Down ? 1
             : event->keyval == GDK_KEY_Up ? -1
                                           : 0;
  if (!step)
    return FALSE;
  GtkListBox *list = GTK_LIST_BOX(n->results);
  g_autoptr(GList) rows = gtk_container_get_children(GTK_CONTAINER(list));
  int count = g_list_length(rows);
  if (!count)
    return TRUE;
  GtkListBoxRow *row = gtk_list_box_get_selected_row(list);
  int index = row ? gtk_list_box_row_get_index(row) + step : 0;
  gtk_list_box_select_row(
      list, gtk_list_box_get_row_at_index(list, (index + count) % count));
  return TRUE;
}

#define PILL_WIDTH 76
#define PILL_HEIGHT 8
#define MORPH_RADIUS 22

typedef struct {
  double from, to, x;
} Tween;

typedef struct {
  GtkBin parent;
  Notch *notch;
  Tween width, height;
  gint64 start;
  guint tick;
} NotchMorph;

typedef struct {
  GtkBinClass parent;
} NotchMorphClass;

G_DEFINE_TYPE(NotchMorph, notch_morph, GTK_TYPE_BIN)

// cubic-bezier(0.32, 0.72, 0, 1): gentle start, long soft landing.
static double ease(double t) {
  if (t <= 0)
    return 0;
  if (t >= 1)
    return 1;
  double lo = 0, hi = 1, u = t;
  for (int i = 0; i < 24; i++) {
    u = (lo + hi) / 2;
    double x = 3 * (1 - u) * (1 - u) * u * 0.32 + u * u * u;
    if (x < t)
      lo = u;
    else
      hi = u;
  }
  return 3 * (1 - u) * (1 - u) * u * 0.72 + 3 * (1 - u) * u * u + u * u * u;
}

static int header_height(Notch *n) {
  int height;
  gtk_widget_get_preferred_height_for_width(n->header, NOTCH_WIDTH, NULL,
                                            &height);
  return MAX(height, PILL_HEIGHT + 1);
}

static double morph_radius(int width, int height) {
  return MIN(MIN(width, height) / 2.0, MORPH_RADIUS);
}

static gboolean morph_tick(GtkWidget *widget, GdkFrameClock *clock,
                           gpointer data G_GNUC_UNUSED) {
  NotchMorph *m = (NotchMorph *)widget;
  double t = (gdk_frame_clock_get_frame_time(clock) - m->start) /
             (MOTION_MS * 1000.0),
         p = ease(t);
  m->width.x = m->width.from + (m->width.to - m->width.from) * p;
  m->height.x = m->height.from + (m->height.to - m->height.from) * p;
  gtk_widget_queue_resize(widget);
  if (t < 1)
    return G_SOURCE_CONTINUE;
  m->tick = 0;
  gtk_widget_set_child_visible(m->notch->body, m->notch->state == NOTCH_OPEN);
  return G_SOURCE_REMOVE;
}

static void morph_retarget(NotchMorph *m) {
  Notch *n = m->notch;
  double width = NOTCH_WIDTH, height;
  if (n->state == NOTCH_HIDDEN) {
    width = n->flow_visible ? 0 : PILL_WIDTH;
    height = n->flow_visible ? 0 : PILL_HEIGHT;
  } else if (n->state == NOTCH_PEEK) {
    height = header_height(n);
  } else {
    int natural;
    gtk_widget_get_preferred_height_for_width(n->surface, NOTCH_WIDTH, NULL,
                                              &natural);
    height = natural;
  }
  if (width == m->width.to && height == m->height.to)
    return;
  m->width = (Tween){m->width.x, width, m->width.x};
  m->height = (Tween){m->height.x, height, m->height.x};
  m->start = g_get_monotonic_time();
  gboolean animate;
  g_object_get(gtk_widget_get_settings(GTK_WIDGET(m)), "gtk-enable-animations",
               &animate, NULL);
  if (!animate || !gtk_widget_get_mapped(GTK_WIDGET(m))) {
    m->width = (Tween){width, width, width};
    m->height = (Tween){height, height, height};
    if (m->tick)
      gtk_widget_remove_tick_callback(GTK_WIDGET(m), m->tick);
    m->tick = 0;
  } else if (!m->tick) {
    m->tick =
        gtk_widget_add_tick_callback(GTK_WIDGET(m), morph_tick, NULL, NULL);
  }
}

static GtkSizeRequestMode morph_request_mode(GtkWidget *widget G_GNUC_UNUSED) {
  return GTK_SIZE_REQUEST_CONSTANT_SIZE;
}

static void morph_width(GtkWidget *widget, int *minimum, int *natural) {
  NotchMorph *m = (NotchMorph *)widget;
  morph_retarget(m);
  // Even widths keep the centred shape from shimmering by half a pixel.
  *minimum = *natural = 2 * (int)(m->width.x / 2 + 0.5);
}

static void morph_height(GtkWidget *widget, int *minimum, int *natural) {
  NotchMorph *m = (NotchMorph *)widget;
  morph_retarget(m);
  *minimum = *natural = (int)(m->height.x + 0.5);
}

static void morph_width_for(GtkWidget *widget, int size G_GNUC_UNUSED,
                            int *minimum, int *natural) {
  morph_width(widget, minimum, natural);
}

static void morph_height_for(GtkWidget *widget, int size G_GNUC_UNUSED,
                             int *minimum, int *natural) {
  morph_height(widget, minimum, natural);
}

static void morph_allocate(GtkWidget *widget, GtkAllocation *box) {
  NotchMorph *m = (NotchMorph *)widget;
  gtk_widget_set_allocation(widget, box);
  GtkWidget *child = gtk_bin_get_child(GTK_BIN(widget));
  if (!child || !gtk_widget_get_visible(child))
    return;
  int height, header = header_height(m->notch);
  gtk_widget_get_preferred_height_for_width(child, NOTCH_WIDTH, NULL, &height);
  // Below peek height the content rides the bottom edge, so the pill stays
  // empty and the header slides down as the shape grows.
  double shown =
      CLAMP((m->height.x - PILL_HEIGHT) / (header - PILL_HEIGHT), 0, 1);
  gtk_widget_size_allocate(
      child, &(GtkAllocation){box->x + (box->width - NOTCH_WIDTH) / 2,
                              box->y - (int)((1 - shown) * header + 0.5),
                              NOTCH_WIDTH, height});
}

static void rounded_path(cairo_t *cr, double x, double y, double w, double h,
                         double r) {
  cairo_new_sub_path(cr);
  cairo_arc(cr, x + w - r, y + r, r, -G_PI / 2, 0);
  cairo_arc(cr, x + w - r, y + h - r, r, 0, G_PI / 2);
  cairo_arc(cr, x + r, y + h - r, r, G_PI / 2, G_PI);
  cairo_arc(cr, x + r, y + r, r, G_PI, 3 * G_PI / 2);
  cairo_close_path(cr);
}

static gboolean morph_draw(GtkWidget *widget, cairo_t *cr) {
  int w = gtk_widget_get_allocated_width(widget),
      h = gtk_widget_get_allocated_height(widget);
  if (w <= 1 || h <= 1)
    return FALSE;
  GtkStyleContext *context = gtk_widget_get_style_context(widget);
  GdkRGBA *fill, *edge;
  int border;
  gtk_style_context_get(context, gtk_style_context_get_state(context),
                        "background-color", &fill, "border-top-color", &edge,
                        "border-top-width", &border, NULL);
  double r = morph_radius(w, h);
  rounded_path(cr, 0, 0, w, h, r);
  gdk_cairo_set_source_rgba(cr, fill);
  cairo_fill_preserve(cr);
  cairo_save(cr);
  cairo_clip(cr);
  GTK_WIDGET_CLASS(notch_morph_parent_class)->draw(widget, cr);
  cairo_restore(cr);
  double inset = border / 2.0;
  if (border && w > border && h > border) {
    rounded_path(cr, inset, inset, w - border, h - border, MAX(r - inset, 0));
    cairo_set_line_width(cr, border);
    gdk_cairo_set_source_rgba(cr, edge);
    cairo_stroke(cr);
  }
  gdk_rgba_free(fill);
  gdk_rgba_free(edge);
  return FALSE;
}

static void notch_morph_class_init(NotchMorphClass *klass) {
  GtkWidgetClass *widget = GTK_WIDGET_CLASS(klass);
  widget->get_request_mode = morph_request_mode;
  widget->get_preferred_width = morph_width;
  widget->get_preferred_height = morph_height;
  widget->get_preferred_width_for_height = morph_width_for;
  widget->get_preferred_height_for_width = morph_height_for;
  widget->size_allocate = morph_allocate;
  widget->draw = morph_draw;
}

static void notch_morph_init(NotchMorph *m) {
  gtk_widget_set_has_window(GTK_WIDGET(m), FALSE);
}

static void morph_update(Notch *n) {
  NotchMorph *m = (NotchMorph *)n->morph;
  if (n->state == NOTCH_OPEN)
    gtk_widget_set_child_visible(n->body, TRUE);
  morph_retarget(m);
  if (!m->tick)
    gtk_widget_set_child_visible(n->body, n->state == NOTCH_OPEN);
  gtk_widget_queue_resize(n->morph);
}

static void set_state(Notch *n, NotchState state) {
  g_clear_handle_id(&n->close_source, g_source_remove);
  g_clear_handle_id(&n->open_source, g_source_remove);
  n->state = state;
  gboolean open = state == NOTCH_OPEN;
  morph_update(n);
  atk_object_set_name(gtk_widget_get_accessible(n->header),
                      open ? "Close notch controls" : "Open notch controls");
  atk_object_notify_state_change(gtk_widget_get_accessible(n->header),
                                 ATK_STATE_EXPANDED, open);
  if (open) {
    level_read(&n->volume);
    level_read(&n->brightness);
  } else {
    gtk_entry_set_text(GTK_ENTRY(n->search), "");
    gtk_layer_set_keyboard_mode(GTK_WINDOW(n->window),
                                n->flow_visible
                                    ? GTK_LAYER_SHELL_KEYBOARD_MODE_NONE
                                    : GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND);
  }
}

static void collapse_clicked(GtkButton *button G_GNUC_UNUSED, gpointer data) {
  Notch *n = data;
  n->pinned = FALSE;
  set_state(n, NOTCH_HIDDEN);
}

static void pin_clicked(GtkButton *button G_GNUC_UNUSED, gpointer data) {
  Notch *n = data;
  n->pinned = !n->pinned;
  set_state(n, n->pinned ? NOTCH_OPEN : NOTCH_HIDDEN);
}

static gboolean close_after_hover(gpointer data) {
  Notch *n = data;
  n->close_source = 0;
  collapse_clicked(NULL, n);
  return G_SOURCE_REMOVE;
}

static gboolean open_after_hover(gpointer data) {
  Notch *n = data;
  n->open_source = 0;
  set_state(n, NOTCH_OPEN);
  return G_SOURCE_REMOVE;
}

static void hover(Notch *n) {
  g_clear_handle_id(&n->close_source, g_source_remove);
  if (n->state == NOTCH_HIDDEN)
    set_state(n, NOTCH_PEEK);
  else if (n->state == NOTCH_PEEK && !n->open_source &&
           !((NotchMorph *)n->morph)->tick)
    n->open_source = g_timeout_add(100, open_after_hover, n);
}

static gboolean crossing(GtkWidget *widget G_GNUC_UNUSED,
                         GdkEventCrossing *event, gpointer data) {
  Notch *n = data;
  if (event->detail == GDK_NOTIFY_INFERIOR)
    return FALSE;
  if (event->type == GDK_ENTER_NOTIFY)
    hover(n);
  else if (!n->pinned && !*gtk_entry_get_text(GTK_ENTRY(n->search))) {
    g_clear_handle_id(&n->open_source, g_source_remove);
    g_clear_handle_id(&n->close_source, g_source_remove);
    n->close_source = g_timeout_add(250, close_after_hover, n);
  }
  return FALSE;
}

static gboolean motion(GtkWidget *widget G_GNUC_UNUSED,
                       GdkEventMotion *event G_GNUC_UNUSED, gpointer data) {
  hover(data);
  return FALSE;
}

static void add_input(cairo_region_t *region, GtkWidget *widget) {
  GtkAllocation box;
  gtk_widget_get_allocation(widget, &box);
  if (box.width > 1 && box.height > 1)
    cairo_region_union_rectangle(
        region, &(cairo_rectangle_int_t){box.x, box.y, box.width, box.height});
}

static void add_rounded(cairo_region_t *region, GtkWidget *widget,
                        GtkWidget *clip, int radius, GtkWidget *window) {
  GtkAllocation view;
  gtk_widget_get_allocation(clip, &view);
  int x, y;
  if (view.height <= 1 ||
      !gtk_widget_translate_coordinates(widget, window, 0, 0, &x, &y))
    return;
  int w = gtk_widget_get_allocated_width(widget),
      h = gtk_widget_get_allocated_height(widget);
  radius = MIN(radius, MIN(w, h) / 2);
  cairo_region_t *shape = cairo_region_create_rectangle(
      &(cairo_rectangle_int_t){x, y + radius, w, h - 2 * radius});
  for (int row = 0; row < radius; row++) {
    double dy = radius - row - 0.5, dx;
    int inset = 0;
    do
      dx = radius - inset - 0.5;
    while (dx * dx + dy * dy > radius * radius && ++inset < radius);
    cairo_region_union_rectangle(
        shape, &(cairo_rectangle_int_t){x + inset, y + row, w - 2 * inset, 1});
    cairo_region_union_rectangle(
        shape,
        &(cairo_rectangle_int_t){x + inset, y + h - 1 - row, w - 2 * inset, 1});
  }
  cairo_region_intersect_rectangle(shape, &view);
  cairo_region_union(region, shape);
  cairo_region_destroy(shape);
}

static void update_blur(Notch *n) {
  if (!n->blur)
    return;
  GtkAllocation box;
  gtk_widget_get_allocation(n->morph, &box);
  cairo_region_t *shape = cairo_region_create();
  if (box.width > 1 && box.height > 1)
    add_rounded(shape, n->morph, n->morph,
                (int)morph_radius(box.width, box.height), n->window);
  if (gtk_widget_get_mapped(n->flow_button))
    add_rounded(shape, n->flow_button, n->flow, G_MAXINT, n->window);
  if (cairo_region_is_empty(shape)) {
    ext_background_effect_surface_v1_set_blur_region(n->blur, NULL);
    cairo_region_destroy(shape);
    return;
  }
  struct wl_region *region = wl_compositor_create_region(
      gdk_wayland_display_get_wl_compositor(gtk_widget_get_display(n->window)));
  for (int i = 0; i < cairo_region_num_rectangles(shape); i++) {
    cairo_rectangle_int_t rect;
    cairo_region_get_rectangle(shape, i, &rect);
    wl_region_add(region, rect.x, rect.y, rect.width, rect.height);
  }
  ext_background_effect_surface_v1_set_blur_region(n->blur, region);
  wl_region_destroy(region);
  cairo_region_destroy(shape);
}

static void registry_global(void *data, struct wl_registry *registry,
                            uint32_t name, const char *interface,
                            uint32_t version G_GNUC_UNUSED) {
  if (!strcmp(interface, ext_background_effect_manager_v1_interface.name))
    *(struct ext_background_effect_manager_v1 **)data = wl_registry_bind(
        registry, name, &ext_background_effect_manager_v1_interface, 1);
}

static void registry_global_remove(void *data G_GNUC_UNUSED,
                                   struct wl_registry *registry G_GNUC_UNUSED,
                                   uint32_t name G_GNUC_UNUSED) {}

static void bind_blur(Notch *n) {
  GdkDisplay *display = gtk_widget_get_display(n->window);
  GdkWindow *window = gtk_widget_get_window(n->window);
  if (!GDK_IS_WAYLAND_DISPLAY(display) || !window)
    return;
  static const struct wl_registry_listener listener = {registry_global,
                                                       registry_global_remove};
  struct wl_display *wl = gdk_wayland_display_get_wl_display(display);
  struct wl_event_queue *queue = wl_display_create_queue(wl);
  struct wl_display *wrapper = wl_proxy_create_wrapper(wl);
  wl_proxy_set_queue((struct wl_proxy *)wrapper, queue);
  struct wl_registry *registry = wl_display_get_registry(wrapper);
  struct ext_background_effect_manager_v1 *manager = NULL;
  wl_registry_add_listener(registry, &listener, &manager);
  wl_display_roundtrip_queue(wl, queue);
  if (manager) {
    wl_proxy_set_queue((struct wl_proxy *)manager, NULL);
    n->blur = ext_background_effect_manager_v1_get_background_effect(
        manager, gdk_wayland_window_get_wl_surface(window));
    ext_background_effect_manager_v1_destroy(manager);
  }
  wl_registry_destroy(registry);
  wl_proxy_wrapper_destroy(wrapper);
  wl_event_queue_destroy(queue);
}

static void update_input(GtkWidget *widget G_GNUC_UNUSED,
                         GdkRectangle *allocation G_GNUC_UNUSED,
                         gpointer data) {
  Notch *n = data;
  GdkWindow *window = gtk_widget_get_window(n->window);
  if (!window)
    return;
  cairo_region_t *region = cairo_region_create();
  if (n->flow)
    add_input(region, n->flow);
  GtkAllocation box;
  gtk_widget_get_allocation(n->morph, &box);
  if (box.width > 1 && box.height > 1) {
    // Pad the thin pill so it is easy to hit.
    if (n->state == NOTCH_HIDDEN)
      box = (GtkAllocation){box.x - 28, box.y, box.width + 56, box.height + 12};
    cairo_region_union_rectangle(
        region, &(cairo_rectangle_int_t){box.x, box.y, box.width, box.height});
  }
  gdk_window_input_shape_combine_region(window, region, 0, 0);
  cairo_region_destroy(region);
  update_blur(n);
}

static gboolean flow_icon_draw(GtkWidget *widget G_GNUC_UNUSED, cairo_t *cr,
                               gpointer data G_GNUC_UNUSED) {
  cairo_scale(cr, 0.75, 0.75);
  cairo_set_source_rgb(cr, 1, 1, 1);
  cairo_set_line_width(cr, 2.5);
  cairo_arc(cr, 20, 20, 17, 0, 2 * G_PI);
  cairo_stroke(cr);
  cairo_set_line_cap(cr, CAIRO_LINE_CAP_ROUND);
  const double heights[] = {8, 16, 10};
  for (guint i = 0; i < G_N_ELEMENTS(heights); i++) {
    cairo_move_to(cr, 14 + i * 6, 20 - heights[i] / 2);
    cairo_line_to(cr, 14 + i * 6, 20 + heights[i] / 2);
  }
  cairo_stroke(cr);
  return FALSE;
}

static gboolean flow_draw(GtkWidget *widget, cairo_t *cr, gpointer data) {
  Notch *n = data;
  const double line = 2.25, pitch = 6.75;
  double w = gtk_widget_get_allocated_width(widget),
         mid = gtk_widget_get_allocated_height(widget) / 2.0;
  guint total = G_N_ELEMENTS(n->flow_levels),
        count = MIN(total, (guint)((w - line) / pitch) + 1);
  if (count < 2)
    return FALSE;
  double step = (w - line) / (count - 1);
  cairo_set_source_rgb(cr, 1, 1, 1);
  cairo_set_line_cap(cr, CAIRO_LINE_CAP_ROUND);
  cairo_set_line_width(cr, line);
  for (guint i = 0; i < count; i++) {
    double height = 3 + 16.5 * n->flow_levels[total - count + i];
    cairo_move_to(cr, line / 2 + i * step, mid - height / 2);
    cairo_line_to(cr, line / 2 + i * step, mid + height / 2);
  }
  cairo_stroke(cr);
  return FALSE;
}

static void flow_activate(Notch *n, const char *action) {
  if (n->flow_actions &&
      g_action_group_has_action(G_ACTION_GROUP(n->flow_actions), action)) {
    g_action_group_activate_action(G_ACTION_GROUP(n->flow_actions), action,
                                   NULL);
  } else {
    g_autoptr(GError) error = NULL;
    if (!g_spawn_command_line_async("hype-flow", &error))
      show_message(n, "Install Hype Flow to use offline dictation.");
  }
}

static void flow_clicked(GtkButton *button, gpointer data) {
  Notch *n = data;
  const char *action = g_object_get_data(G_OBJECT(button), "flow-action");
  flow_activate(n, action ? action : n->flow_recording ? "toggle" : "show");
}

static gboolean flow_pressed(GtkWidget *widget G_GNUC_UNUSED,
                             GdkEventButton *event, gpointer data) {
  if (event->button != 3)
    return FALSE;
  flow_activate(data, "cancel");
  return TRUE;
}

static void flow_update(Notch *n, GVariant *value) {
  if (!g_variant_is_of_type(value, G_VARIANT_TYPE("(ssddb)")))
    return;
  const char *state, *message;
  double level, seconds;
  gboolean visible;
  g_variant_get(value, "(&s&sddb)", &state, &message, &level, &seconds,
                &visible);
  gboolean was_visible = n->flow_visible;
  n->flow_visible = visible;
  n->flow_recording = !strcmp(state, "recording") || !strcmp(state, "starting");
  memmove(n->flow_levels, n->flow_levels + 1,
          sizeof(n->flow_levels) - sizeof(n->flow_levels[0]));
  n->flow_levels[G_N_ELEMENTS(n->flow_levels) - 1] =
      level >= 0 && level <= 1 ? level : 0;
  gtk_widget_set_visible(n->flow_meter, n->flow_recording);
  gtk_widget_queue_draw(n->flow_meter);
  gtk_label_set_text(GTK_LABEL(n->flow_status), message);
  const char *caption = !strcmp(state, "error")        ? "Needs attention"
                        : !strcmp(state, "done")       ? message
                        : !strcmp(state, "loading")    ? "Loading model"
                        : !strcmp(state, "delivering") ? "Copying"
                                                       : "Transcribing";
  gtk_label_set_text(GTK_LABEL(n->flow_label), caption);
  gtk_widget_set_visible(n->flow_label, !n->flow_recording);
  set_button_icon(n->flow_toggle,
                  n->flow_recording ? "media-playback-stop-symbolic"
                                    : "audio-input-microphone-symbolic",
                  n->flow_recording ? "Finish dictation" : "Start dictation");
  gboolean idle = !strcmp(state, "idle") || !strcmp(state, "done") ||
                  !strcmp(state, "error");
  gtk_widget_set_sensitive(n->flow_toggle, idle || n->flow_recording);
  gtk_widget_set_sensitive(n->flow_cancel, !idle);
  const char *tip = n->flow_recording
                        ? "Listening: click to finish, right-click to cancel"
                        : message;
  gtk_widget_set_tooltip_text(n->flow_button, tip);
  atk_object_set_name(gtk_widget_get_accessible(n->flow_button), tip);
  gtk_revealer_set_reveal_child(GTK_REVEALER(n->flow), visible);
  morph_update(n);
  if (visible || was_visible)
    gtk_layer_set_keyboard_mode(GTK_WINDOW(n->window),
                                visible
                                    ? GTK_LAYER_SHELL_KEYBOARD_MODE_NONE
                                    : GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND);
  (void)seconds;
}

static void flow_changed(GActionGroup *group G_GNUC_UNUSED, const char *name,
                         GVariant *value, gpointer data) {
  if (!strcmp(name, "status"))
    flow_update(data, value);
}

static void flow_added(GActionGroup *group, const char *name, gpointer data) {
  if (strcmp(name, "status"))
    return;
  g_autoptr(GVariant) value = g_action_group_get_action_state(group, name);
  if (value)
    flow_update(data, value);
}

static void flow_appeared(GDBusConnection *connection, const char *name,
                          const char *owner G_GNUC_UNUSED, gpointer data) {
  Notch *n = data;
  g_clear_object(&n->flow_actions);
  n->flow_actions = g_dbus_action_group_get(connection, name, "/dev/hype/Flow");
  g_signal_connect(n->flow_actions, "action-state-changed",
                   G_CALLBACK(flow_changed), n);
  g_signal_connect(n->flow_actions, "action-added", G_CALLBACK(flow_added), n);
  flow_added(G_ACTION_GROUP(n->flow_actions), "status", n);
  gtk_widget_show(n->flow_card);
}

static void flow_vanished(GDBusConnection *connection G_GNUC_UNUSED,
                          const char *name G_GNUC_UNUSED, gpointer data) {
  Notch *n = data;
  g_clear_object(&n->flow_actions);
  g_autoptr(GVariant) state = g_variant_ref_sink(g_variant_new(
      "(ssddb)", "idle", "Open Hype Flow to start dictation", 0.0, 0.0, FALSE));
  flow_update(n, state);
}

static GtkWidget *build_flow(Notch *n) {
  n->flow = revealer(GTK_REVEALER_TRANSITION_TYPE_SLIDE_DOWN, FALSE);
  gtk_widget_set_halign(n->flow, GTK_ALIGN_CENTER);

  n->flow_button = gtk_button_new();
  gtk_widget_set_focus_on_click(n->flow_button, FALSE);
  gtk_widget_set_name(n->flow_button, "notch-dictation");
  // Widget margins sit outside the allocation, so the blur region matches.
  gtk_widget_set_margin_top(n->flow_button, 2);
  gtk_widget_set_margin_bottom(n->flow_button, 8);
  g_signal_connect(n->flow_button, "clicked", G_CALLBACK(flow_clicked), n);
  g_signal_connect(n->flow_button, "button-press-event",
                   G_CALLBACK(flow_pressed), n);
  gtk_widget_set_size_request(n->flow_button, NOTCH_WIDTH, -1);
  GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
  GtkWidget *icon = gtk_drawing_area_new();
  gtk_widget_set_size_request(icon, 30, 30);
  g_signal_connect(icon, "draw", G_CALLBACK(flow_icon_draw), n);
  n->flow_meter = gtk_drawing_area_new();
  gtk_widget_set_size_request(n->flow_meter, 40, 30);
  g_signal_connect(n->flow_meter, "draw", G_CALLBACK(flow_draw), n);
  n->flow_label = text_label("", NULL);
  pack(row, icon, FALSE);
  pack(row, n->flow_label, TRUE);
  pack(row, n->flow_meter, TRUE);
  gtk_container_add(GTK_CONTAINER(n->flow_button), row);
  gtk_container_add(GTK_CONTAINER(n->flow), n->flow_button);
  return n->flow;
}

static GtkWidget *build_flow_controls(Notch *n) {
  n->flow_card = card("notch-dictation-controls");
  GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
  const char *icons[] = {"audio-input-microphone-symbolic",
                         "process-stop-symbolic", "edit-copy-symbolic",
                         "preferences-system-symbolic"};
  const char *names[] = {"Start dictation", "Cancel dictation",
                         "Copy last transcript", "Hype Flow settings"};
  const char *actions[] = {"toggle", "cancel", "copy-last", "show"};
  for (guint i = 0; i < G_N_ELEMENTS(actions); i++) {
    GtkWidget *button =
        icon_button(icons[i], names[i], G_CALLBACK(flow_clicked), n);
    g_object_set_data(G_OBJECT(button), "flow-action", (gpointer)actions[i]);
    pack(row, button, FALSE);
    if (i == 1)
      n->flow_cancel = button;
    if (i)
      continue;
    n->flow_toggle = button;
    add_class(button, "primary");
    GtkWidget *text = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    gtk_widget_set_valign(text, GTK_ALIGN_CENTER);
    gtk_widget_set_margin_start(text, 4);
    pack(text, text_label("Hype Flow", "strong"), FALSE);
    n->flow_status = text_label("Open Hype Flow to start dictation", "muted");
    pack(text, n->flow_status, FALSE);
    pack(row, text, TRUE);
  }
  gtk_widget_set_sensitive(n->flow_cancel, FALSE);
  pack(n->flow_card, row, FALSE);
  gtk_widget_show_all(n->flow_card);
  gtk_widget_set_no_show_all(n->flow_card, TRUE);
  g_autofree char *launcher = g_find_program_in_path("hype-flow");
  gtk_widget_set_visible(n->flow_card, launcher != NULL);
  return n->flow_card;
}

static gboolean key_pressed(GtkWidget *widget G_GNUC_UNUSED, GdkEventKey *event,
                            gpointer data) {
  Notch *n = data;
  if (event->keyval != GDK_KEY_Escape)
    return FALSE;
  if (*gtk_entry_get_text(GTK_ENTRY(n->search)))
    gtk_entry_set_text(GTK_ENTRY(n->search), "");
  else
    collapse_clicked(NULL, n);
  return TRUE;
}

static gboolean focus_out(GtkWidget *widget G_GNUC_UNUSED,
                          GdkEventFocus *event G_GNUC_UNUSED, gpointer data) {
  collapse_clicked(NULL, data);
  return FALSE;
}

static gboolean wave_tick(GtkWidget *wave, GdkFrameClock *clock G_GNUC_UNUSED,
                          gpointer data G_GNUC_UNUSED) {
  gtk_widget_queue_draw(wave);
  return G_SOURCE_CONTINUE;
}

static gboolean wave_draw(GtkWidget *wave, cairo_t *cr,
                          gpointer data G_GNUC_UNUSED) {
  GtkStyleContext *style = gtk_widget_get_style_context(wave);
  GdkRGBA color;
  gtk_style_context_get_color(style, gtk_style_context_get_state(style),
                              &color);
  gdk_cairo_set_source_rgba(cr, &color);
  GdkFrameClock *clock = gtk_widget_get_frame_clock(wave);
  double now = clock ? gdk_frame_clock_get_frame_time(clock) / 1e6 : 0;
  int height = gtk_widget_get_allocated_height(wave);
  cairo_set_line_width(cr, 3);
  cairo_set_line_cap(cr, CAIRO_LINE_CAP_ROUND);
  for (int i = 0; i < 4; i++) {
    double phase = now * (1.1 + 0.25 * i) + 0.31 * i;
    phase -= (gint64)phase;
    double level = phase < 0.5 ? phase * 2 : 2 - phase * 2;
    level = level * level * (3 - 2 * level);
    double bar = (height - 4) * (0.3 + 0.7 * level);
    cairo_move_to(cr, 2.5 + i * 5, (height - bar) / 2);
    cairo_line_to(cr, 2.5 + i * 5, (height + bar) / 2);
  }
  cairo_stroke(cr);
  return FALSE;
}

static GtkWidget *wave_new(void) {
  GtkWidget *wave = gtk_drawing_area_new();
  gtk_widget_set_size_request(wave, 18, 16);
  gtk_widget_set_valign(wave, GTK_ALIGN_CENTER);
  add_class(wave, "wave");
  g_signal_connect(wave, "draw", G_CALLBACK(wave_draw), NULL);
  return wave;
}

static void set_wave(GtkWidget *wave, gboolean playing) {
  guint tick = GPOINTER_TO_UINT(g_object_get_data(G_OBJECT(wave), "tick"));
  gboolean animate;
  g_object_get(gtk_widget_get_settings(wave), "gtk-enable-animations", &animate,
               NULL);
  if (playing && animate && !tick)
    tick = gtk_widget_add_tick_callback(wave, wave_tick, NULL, NULL);
  else if ((!playing || !animate) && tick) {
    gtk_widget_remove_tick_callback(wave, tick);
    tick = 0;
  }
  g_object_set_data(G_OBJECT(wave), "tick", GUINT_TO_POINTER(tick));
  gtk_widget_set_visible(wave, playing);
}

static void refresh_media(Notch *n) {
  GList *players = NULL;
  g_object_get(n->manager, "players", &players, NULL);
  PlayerctlPlayer *selected = players ? players->data : NULL;
  for (GList *item = players; item; item = item->next) {
    PlayerctlPlaybackStatus status;
    g_object_get(item->data, "playback-status", &status, NULL);
    if (status == PLAYERCTL_PLAYBACK_STATUS_PLAYING) {
      selected = item->data;
      break;
    }
  }
  g_set_object(&n->player, selected);
  g_autofree char *title = NULL, *artist = NULL, *name = NULL;
  gboolean can_previous = FALSE, can_next = FALSE, can_play = FALSE,
           can_pause = FALSE;
  PlayerctlPlaybackStatus status = PLAYERCTL_PLAYBACK_STATUS_STOPPED;
  if (n->player) {
    g_object_get(n->player, "player-name", &name, "playback-status", &status,
                 "can-go-previous", &can_previous, "can-go-next", &can_next,
                 "can-play", &can_play, "can-pause", &can_pause, NULL);
    title = playerctl_player_get_title(n->player, NULL);
    artist = playerctl_player_get_artist(n->player, NULL);
  }
  const char *title_text = title && *title ? title
                           : name          ? name
                                           : "Nothing playing";
  const char *artist_text = artist && *artist ? artist
                            : name
                                ? name
                                : "Start a song or video in your media player";
  gtk_label_set_text(GTK_LABEL(n->title), title_text);
  gtk_label_set_text(GTK_LABEL(n->artist), artist_text);
  gboolean playing = status == PLAYERCTL_PLAYBACK_STATUS_PLAYING;
  // A player that cannot play or pause (an idle browser tab) is not media.
  gboolean active = n->player && (playing || can_play || can_pause);
  gtk_label_set_text(GTK_LABEL(n->summary),
                     active ? title_text : "No media playing");
  set_class(n->summary, "muted", !active);
  gtk_widget_set_visible(n->media_card, active);
  gtk_widget_set_visible(n->media_icon, !playing);
  set_wave(n->header_wave, playing);
  set_wave(n->card_wave, playing);
  set_button_icon(n->play,
                  playing ? "media-playback-pause-symbolic"
                          : "media-playback-start-symbolic",
                  playing ? "Pause" : "Play");
  gtk_widget_set_sensitive(n->previous, can_previous);
  gtk_widget_set_sensitive(n->next, can_next);
  gtk_widget_set_sensitive(n->play, playing ? can_pause : can_play);
}

static gboolean media_idle(gpointer data) {
  Notch *n = data;
  n->media_source = 0;
  refresh_media(n);
  return G_SOURCE_REMOVE;
}

static void queue_media_refresh(Notch *n) {
  // PlayerManager removes a vanished player after emitting its signal.
  if (!n->media_source)
    n->media_source = g_idle_add(media_idle, n);
}

static void add_player(PlayerctlPlayerManager *manager,
                       PlayerctlPlayerName *name, gpointer data) {
  Notch *n = data;
  g_autoptr(GError) error = NULL;
  PlayerctlPlayer *player = playerctl_player_new_from_name(name, &error);
  if (!player) {
    g_printerr("Notch: %s\n", error->message);
    return;
  }
  g_signal_connect_swapped(player, "metadata", G_CALLBACK(queue_media_refresh),
                           n);
  g_signal_connect_swapped(player, "playback-status",
                           G_CALLBACK(queue_media_refresh), n);
  playerctl_player_manager_manage_player(manager, player);
  g_object_unref(player);
  queue_media_refresh(n);
}

static void media_clicked(GtkButton *button, gpointer data) {
  Notch *n = data;
  if (!n->player)
    return;
  g_autoptr(GError) error = NULL;
  if (GTK_WIDGET(button) == n->previous)
    playerctl_player_previous(n->player, &error);
  else if (GTK_WIDGET(button) == n->next)
    playerctl_player_next(n->player, &error);
  else
    playerctl_player_play_pause(n->player, &error);
  if (error)
    show_message(n, error->message);
}

static gboolean load_style(const char *scripts, GError **error) {
  g_autofree char *path = g_build_filename(scripts, "notch.css", NULL);
  g_autoptr(GtkCssProvider) provider = gtk_css_provider_new();
  if (!gtk_css_provider_load_from_path(provider, path, error))
    return FALSE;
  GdkScreen *screen = gdk_screen_get_default();
  if (screen)
    gtk_style_context_add_provider_for_screen(
        screen, GTK_STYLE_PROVIDER(provider),
        GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
  return TRUE;
}

static GtkWidget *build_header(Notch *n) {
  n->header = gtk_button_new();
  // Mod+N and Escape cover it; as the first focusable widget it would
  // otherwise take initial focus and draw a ring once a key is pressed.
  gtk_widget_set_can_focus(n->header, FALSE);
  gtk_widget_set_name(n->header, "notch-header");
  atk_object_set_name(gtk_widget_get_accessible(n->header),
                      "Open notch controls");
  g_signal_connect(n->header, "clicked", G_CALLBACK(pin_clicked), n);
  GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
  n->media_icon = gtk_image_new_from_icon_name("audio-x-generic-symbolic",
                                               GTK_ICON_SIZE_MENU);
  pack(row, n->media_icon, FALSE);
  n->header_wave = wave_new();
  pack(row, n->header_wave, FALSE);
  n->summary = text_label("No media playing", NULL);
  gtk_label_set_max_width_chars(GTK_LABEL(n->summary), 25);
  pack(row, n->summary, TRUE);
  n->compact_timer = text_label("", "chip");
  gtk_widget_set_no_show_all(n->compact_timer, TRUE);
  pack(row, n->compact_timer, FALSE);
  GtkWidget *weather = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
  add_class(weather, "glance");
  n->weather_icon = gtk_image_new_from_icon_name("weather-overcast-symbolic",
                                                 GTK_ICON_SIZE_MENU);
  n->weather_temp = text_label("--", NULL);
  pack(weather, n->weather_icon, FALSE);
  pack(weather, n->weather_temp, FALSE);
  pack(row, weather, FALSE);
  gtk_container_add(GTK_CONTAINER(n->header), row);
  return n->header;
}

static GtkWidget *build_search(Notch *n) {
  GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
  n->search = gtk_entry_new();
  gtk_widget_set_name(n->search, "notch-search");
  gtk_entry_set_placeholder_text(GTK_ENTRY(n->search),
                                 "Search apps or the web");
  gtk_entry_set_icon_from_icon_name(
      GTK_ENTRY(n->search), GTK_ENTRY_ICON_PRIMARY, "system-search-symbolic");
  atk_object_set_name(gtk_widget_get_accessible(n->search), "Quick search");
  g_signal_connect(n->search, "changed", G_CALLBACK(search_changed), n);
  g_signal_connect(n->search, "activate", G_CALLBACK(search_activate), n);
  g_signal_connect(n->search, "key-press-event", G_CALLBACK(search_key), n);
  pack(box, n->search, FALSE);
  n->results = gtk_list_box_new();
  gtk_widget_set_name(n->results, "notch-results");
  gtk_widget_set_no_show_all(n->results, TRUE);
  g_signal_connect(n->results, "row-activated", G_CALLBACK(launch_row), n);
  pack(box, n->results, FALSE);
  return box;
}

static GtkWidget *build_weather(Notch *n) {
  GtkWidget *box = card("notch-weather");
  GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
  n->forecast_icon = gtk_image_new_from_icon_name("weather-overcast-symbolic",
                                                  GTK_ICON_SIZE_DND);
  gtk_image_set_pixel_size(GTK_IMAGE(n->forecast_icon), 28);
  pack(row, n->forecast_icon, FALSE);
  n->forecast_temp = text_label("--", "temp");
  pack(row, n->forecast_temp, FALSE);
  GtkWidget *text = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
  gtk_widget_set_valign(text, GTK_ALIGN_CENTER);
  GtkWidget *line = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
  n->forecast_condition = text_label("Loading weather", "strong");
  n->forecast_place = text_label("", "muted");
  gtk_label_set_xalign(GTK_LABEL(n->forecast_place), 1);
  pack(line, n->forecast_condition, TRUE);
  pack(line, n->forecast_place, FALSE);
  n->forecast_detail = text_label("", "muted");
  pack(text, line, FALSE);
  pack(text, n->forecast_detail, FALSE);
  pack(row, text, TRUE);
  pack(box, row, FALSE);
  return box;
}

static GtkWidget *level_row(Level *level, const char *icon, const char *name,
                            double min) {
  level->row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
  pack(level->row, gtk_image_new_from_icon_name(icon, GTK_ICON_SIZE_MENU),
       FALSE);
  level->scale =
      gtk_scale_new_with_range(GTK_ORIENTATION_HORIZONTAL, min, 100, 1);
  gtk_scale_set_draw_value(GTK_SCALE(level->scale), FALSE);
  gtk_widget_set_focus_on_click(level->scale, FALSE);
  atk_object_set_name(gtk_widget_get_accessible(level->scale), name);
  pack(level->row, level->scale, TRUE);
  level->value = text_label("", "level-value");
  gtk_label_set_xalign(GTK_LABEL(level->value), 1);
  gtk_label_set_width_chars(GTK_LABEL(level->value), 4);
  pack(level->row, level->value, FALSE);
  g_signal_connect(level->scale, "value-changed", G_CALLBACK(level_changed),
                   level);
  return level->row;
}

static GtkWidget *build_levels(Notch *n) {
  GtkWidget *box = card("notch-levels");
  n->volume =
      (Level){.read = {"wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"},
              .set = {"wpctl", "set-volume", "-l", "1", "@DEFAULT_AUDIO_SINK@"},
              .parse = parse_volume,
              .cancel = n->cancel};
  n->brightness = (Level){.read = {"brightnessctl", "-m"},
                          .set = {"brightnessctl", "-q", "set"},
                          .parse = parse_brightness,
                          .cancel = n->cancel};
  pack(box, level_row(&n->volume, "audio-volume-high-symbolic", "Volume", 0),
       FALSE);
  pack(
      box,
      level_row(&n->brightness, "display-brightness-symbolic", "Brightness", 1),
      FALSE);
  return box;
}

static GtkWidget *build_media(Notch *n) {
  GtkWidget *box = n->media_card = card("notch-media");
  gtk_widget_set_no_show_all(box, TRUE);
  n->title = text_label("Nothing playing", "track-title");
  n->artist = text_label("Start a song or video in your media player", "muted");
  gtk_label_set_max_width_chars(GTK_LABEL(n->title), 35);
  gtk_label_set_max_width_chars(GTK_LABEL(n->artist), 42);
  GtkWidget *heading = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
  pack(heading, n->title, TRUE);
  n->card_wave = wave_new();
  pack(heading, n->card_wave, FALSE);
  pack(box, heading, FALSE);
  pack(box, n->artist, FALSE);
  GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 12);
  gtk_widget_set_halign(row, GTK_ALIGN_CENTER);
  n->previous = icon_button("media-skip-backward-symbolic", "Previous track",
                            G_CALLBACK(media_clicked), n);
  n->play = icon_button("media-playback-start-symbolic", "Play",
                        G_CALLBACK(media_clicked), n);
  add_class(n->play, "primary");
  n->next = icon_button("media-skip-forward-symbolic", "Next track",
                        G_CALLBACK(media_clicked), n);
  pack(row, n->previous, FALSE);
  pack(row, n->play, FALSE);
  pack(row, n->next, FALSE);
  pack(box, row, FALSE);
  return box;
}

static gboolean ring_draw(GtkWidget *ring, cairo_t *cr, gpointer data) {
  Notch *n = data;
  FocusTimer *timer = &n->shared->timer;
  gint64 minute = 60 * G_USEC_PER_SEC;
  double fraction = ring == n->ring
                        ? (double)timer->remaining / timer_duration(timer)
                        : (double)(stopwatch_elapsed(&n->shared->stopwatch,
                                                     g_get_monotonic_time()) %
                                   minute) /
                              minute;
  double w = gtk_widget_get_allocated_width(ring),
         h = gtk_widget_get_allocated_height(ring), line = 3,
         r = MIN(w, h) / 2 - line;
  GtkStyleContext *style = gtk_widget_get_style_context(ring);
  GdkRGBA color;
  gtk_style_context_get_color(style, gtk_style_context_get_state(style),
                              &color);
  cairo_set_line_width(cr, line);
  cairo_set_line_cap(cr, CAIRO_LINE_CAP_ROUND);
  cairo_set_source_rgba(cr, color.red, color.green, color.blue, 0.15);
  cairo_arc(cr, w / 2, h / 2, r, 0, 2 * G_PI);
  cairo_stroke(cr);
  if (fraction <= 0)
    return FALSE;
  gdk_cairo_set_source_rgba(cr, &color);
  cairo_arc(cr, w / 2, h / 2, r, -G_PI / 2, -G_PI / 2 + 2 * G_PI * fraction);
  cairo_stroke(cr);
  return FALSE;
}

static GtkWidget *timer_page(GtkWidget **ring, GtkWidget **value,
                             GtkWidget **state, GtkWidget **button,
                             GtkWidget *left, GtkWidget *reset,
                             const char *start_name, GCallback start, Notch *n,
                             const char *initial, const char *label) {
  GtkWidget *page = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
  GtkWidget *dial = gtk_overlay_new();
  gtk_widget_set_halign(dial, GTK_ALIGN_CENTER);
  *ring = gtk_drawing_area_new();
  gtk_widget_set_size_request(*ring, 96, 96);
  add_class(*ring, "ring");
  g_signal_connect(*ring, "draw", G_CALLBACK(ring_draw), n);
  gtk_container_add(GTK_CONTAINER(dial), *ring);
  GtkWidget *text = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
  gtk_widget_set_halign(text, GTK_ALIGN_CENTER);
  gtk_widget_set_valign(text, GTK_ALIGN_CENTER);
  *value = text_label(initial, "ring-time");
  *state = text_label(label, "muted");
  gtk_label_set_xalign(GTK_LABEL(*value), 0.5);
  gtk_label_set_xalign(GTK_LABEL(*state), 0.5);
  pack(text, *value, FALSE);
  pack(text, *state, FALSE);
  gtk_overlay_add_overlay(GTK_OVERLAY(dial), text);
  pack(page, dial, FALSE);
  GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 14);
  gtk_widget_set_halign(row, GTK_ALIGN_CENTER);
  *button = gtk_button_new();
  gtk_widget_set_focus_on_click(*button, FALSE);
  add_class(*button, "primary");
  add_class(*button, "large");
  set_button_icon(*button, "media-playback-start-symbolic", start_name);
  gtk_widget_set_valign(*button, GTK_ALIGN_CENTER);
  g_signal_connect(*button, "clicked", start, n);
  pack(row, left, FALSE);
  pack(row, *button, FALSE);
  pack(row, reset, FALSE);
  pack(page, row, FALSE);
  return page;
}

static GtkWidget *build_timers(Notch *n) {
  GtkWidget *box = card("notch-timers");
  GtkWidget *stack = gtk_stack_new();
  gtk_stack_set_transition_type(GTK_STACK(stack),
                                GTK_STACK_TRANSITION_TYPE_SLIDE_LEFT_RIGHT);
  gtk_stack_set_transition_duration(GTK_STACK(stack), MOTION_MS);
  GtkWidget *focus = timer_page(
      &n->ring, &n->countdown, &n->phase, &n->timer_button,
      icon_button("media-skip-forward-symbolic", "Skip to next phase",
                  G_CALLBACK(skip_clicked), n),
      icon_button("view-refresh-symbolic", "Reset focus timer",
                  G_CALLBACK(reset_clicked), n),
      "Start focus timer", G_CALLBACK(timer_clicked), n, "25:00", "Focus");
  gtk_stack_add_titled(GTK_STACK(stack), focus, "pomodoro", "Pomodoro");
  // Holds the left slot open so the stopwatch controls stay centred.
  GtkWidget *spacer = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
  gtk_widget_set_size_request(spacer, 34, 34);
  gtk_stack_add_titled(
      GTK_STACK(stack),
      timer_page(&n->watch_ring, &n->watch, &n->watch_state, &n->watch_button,
                 spacer,
                 icon_button("view-refresh-symbolic", "Reset stopwatch",
                             G_CALLBACK(watch_reset_clicked), n),
                 "Start stopwatch", G_CALLBACK(watch_clicked), n, "00:00.0",
                 "Stopwatch"),
      "stopwatch", "Stopwatch");
  GtkWidget *switcher = gtk_stack_switcher_new();
  gtk_stack_switcher_set_stack(GTK_STACK_SWITCHER(switcher), GTK_STACK(stack));
  gtk_box_set_homogeneous(GTK_BOX(switcher), TRUE);
  g_autoptr(GList) tabs = gtk_container_get_children(GTK_CONTAINER(switcher));
  for (GList *tab = tabs; tab; tab = tab->next)
    gtk_widget_set_focus_on_click(tab->data, FALSE);
  pack(box, switcher, FALSE);
  pack(box, stack, FALSE);
  return box;
}

static GtkWidget *revealer(GtkRevealerTransitionType type, gboolean shown) {
  GtkWidget *revealer = gtk_revealer_new();
  gtk_revealer_set_transition_type(GTK_REVEALER(revealer), type);
  gtk_revealer_set_transition_duration(GTK_REVEALER(revealer), MOTION_MS);
  gtk_revealer_set_reveal_child(GTK_REVEALER(revealer), shown);
  return revealer;
}

static void view_cleanup(Notch *n);

static void view_unrealize(GtkWidget *window G_GNUC_UNUSED, gpointer data) {
  Notch *n = data;
  // The blur object must go before its wl_surface does.
  g_clear_pointer(&n->blur, ext_background_effect_surface_v1_destroy);
}

static void view_destroyed(GtkWidget *window G_GNUC_UNUSED, gpointer data) {
  Notch *n = data;
  g_ptr_array_remove(n->shared->views, n);
  view_cleanup(n);
  g_free(n);
}

static Notch *view_new(NotchApp *a, GdkMonitor *monitor) {
  GtkApplication *app = a->app;
  g_autoptr(GError) error = NULL;
  PlayerctlPlayerManager *manager = playerctl_player_manager_new(&error);
  if (!manager) {
    g_printerr("Notch media: %s\n", error->message);
    return NULL;
  }
  Notch *n = g_new0(Notch, 1);
  n->shared = a;
  n->monitor = monitor;
  n->manager = manager;
  n->cancel = g_cancellable_new();
  n->window = gtk_application_window_new(app);
  gtk_window_set_title(GTK_WINDOW(n->window), "Niri Notch");
  gtk_widget_set_name(n->window, "niri-notch");
  gtk_window_set_decorated(GTK_WINDOW(n->window), FALSE);
  gtk_window_set_resizable(GTK_WINDOW(n->window), FALSE);
  gtk_widget_set_size_request(n->window, NOTCH_WIDTH, NOTCH_HEIGHT);
  gtk_widget_set_app_paintable(n->window, TRUE);
  gtk_widget_set_visual(
      n->window, gdk_screen_get_rgba_visual(gtk_widget_get_screen(n->window)));
  gtk_layer_init_for_window(GTK_WINDOW(n->window));
  gtk_layer_set_namespace(GTK_WINDOW(n->window), "hype-notch");
  gtk_layer_set_monitor(GTK_WINDOW(n->window), monitor);
  gtk_layer_set_layer(GTK_WINDOW(n->window), GTK_LAYER_SHELL_LAYER_TOP);
  gtk_layer_set_anchor(GTK_WINDOW(n->window), GTK_LAYER_SHELL_EDGE_TOP, TRUE);
  // Ignore Waybar's exclusive zone; 48 logical pixels clears this repo's bar.
  gtk_layer_set_exclusive_zone(GTK_WINDOW(n->window), -1);
  gtk_layer_set_margin(GTK_WINDOW(n->window), GTK_LAYER_SHELL_EDGE_TOP, 48);
  gtk_layer_set_keyboard_mode(GTK_WINDOW(n->window),
                              GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND);
  gtk_widget_add_events(n->window, GDK_ENTER_NOTIFY_MASK |
                                       GDK_LEAVE_NOTIFY_MASK |
                                       GDK_POINTER_MOTION_MASK);
  g_signal_connect(n->window, "motion-notify-event", G_CALLBACK(motion), n);
  g_signal_connect(n->window, "enter-notify-event", G_CALLBACK(crossing), n);
  g_signal_connect(n->window, "leave-notify-event", G_CALLBACK(crossing), n);
  g_signal_connect(n->window, "key-press-event", G_CALLBACK(key_pressed), n);
  g_signal_connect(n->window, "focus-out-event", G_CALLBACK(focus_out), n);

  GtkWidget *stage = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
  gtk_widget_set_valign(stage, GTK_ALIGN_START);
  g_signal_connect_after(stage, "size-allocate", G_CALLBACK(update_input), n);
  gtk_container_add(GTK_CONTAINER(n->window), stage);
  pack(stage, build_flow(n), FALSE);
  n->morph = g_object_new(notch_morph_get_type(), NULL);
  ((NotchMorph *)n->morph)->notch = n;
  gtk_widget_set_name(n->morph, "notch-morph");
  gtk_widget_set_halign(n->morph, GTK_ALIGN_CENTER);
  pack(stage, n->morph, FALSE);
  n->surface = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
  gtk_widget_set_name(n->surface, "notch-surface");
  gtk_container_add(GTK_CONTAINER(n->morph), n->surface);
  pack(n->surface, build_header(n), FALSE);
  GtkWidget *body = n->body = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
  gtk_widget_set_name(body, "notch-body");
  pack(n->surface, body, FALSE);
  pack(body, build_search(n), FALSE);
  pack(body, build_flow_controls(n), FALSE);
  pack(body, build_weather(n), FALSE);
  pack(body, build_levels(n), FALSE);
  pack(body, build_media(n), FALSE);
  pack(body, build_timers(n), FALSE);
  n->message = text_label("", "muted");
  gtk_label_set_line_wrap(GTK_LABEL(n->message), TRUE);
  gtk_label_set_ellipsize(GTK_LABEL(n->message), PANGO_ELLIPSIZE_NONE);
  gtk_label_set_max_width_chars(GTK_LABEL(n->message), 40);
  gtk_widget_set_no_show_all(n->message, TRUE);
  pack(body, n->message, FALSE);

  g_signal_connect(n->manager, "name-appeared", G_CALLBACK(add_player), n);
  g_signal_connect_swapped(n->manager, "player-vanished",
                           G_CALLBACK(queue_media_refresh), n);
  GList *names = NULL;
  g_object_get(n->manager, "player-names", &names, NULL);
  for (GList *item = names; item; item = item->next)
    add_player(n->manager, item->data, n);
  gtk_widget_show_all(n->window);
  refresh_media(n);
  bind_blur(n);
  update_input(NULL, NULL, n);
  level_read(&n->volume);
  level_read(&n->brightness);
  fetch_weather(n);
  n->weather_source = g_timeout_add_seconds(1800, fetch_weather, n);
  n->flow_watch = g_bus_watch_name(G_BUS_TYPE_SESSION, "dev.hype.Flow",
                                   G_BUS_NAME_WATCHER_FLAGS_NONE, flow_appeared,
                                   flow_vanished, n, NULL);
  g_signal_connect(n->window, "unrealize", G_CALLBACK(view_unrealize), n);
  g_signal_connect(n->window, "destroy", G_CALLBACK(view_destroyed), n);
  g_ptr_array_add(a->views, n);
  sync_clocks(a);
  return n;
}

static void monitor_added(GdkDisplay *display G_GNUC_UNUSED,
                          GdkMonitor *monitor, gpointer data) {
  view_new(data, monitor);
}

static void monitor_removed(GdkDisplay *display G_GNUC_UNUSED,
                            GdkMonitor *monitor, gpointer data) {
  NotchApp *a = data;
  for (guint i = 0; i < a->views->len; i++) {
    Notch *n = g_ptr_array_index(a->views, i);
    if (n->monitor == monitor) {
      gtk_widget_destroy(n->window);
      return;
    }
  }
}

static void activate(GtkApplication *app, gpointer data) {
  NotchApp *a = data;
  if (a->views)
    return;
  g_autoptr(GError) error = NULL;
  if (!load_style(a->scripts, &error)) {
    g_printerr("Notch style: %s\n", error->message);
    g_application_quit(G_APPLICATION(app));
    return;
  }
  a->views = g_ptr_array_new();
  GdkDisplay *display = gdk_display_get_default();
  for (int i = 0; i < gdk_display_get_n_monitors(display); i++)
    if (!view_new(a, gdk_display_get_monitor(display, i))) {
      g_application_quit(G_APPLICATION(app));
      return;
    }
  // Stay alive with no monitors connected; views return on monitor-added.
  g_application_hold(G_APPLICATION(app));
  g_signal_connect(display, "monitor-added", G_CALLBACK(monitor_added), a);
  g_signal_connect(display, "monitor-removed", G_CALLBACK(monitor_removed), a);
  GSimpleAction *capability = g_simple_action_new_stateful(
      "dictation", NULL, g_variant_new_boolean(TRUE));
  g_action_map_add_action(G_ACTION_MAP(app), G_ACTION(capability));
  g_object_unref(capability);
}

static Notch *focused_view(NotchApp *a) {
  Notch *first = g_ptr_array_index(a->views, 0);
  g_autofree char *out = NULL;
  int x, y;
  if (!g_spawn_command_line_sync("niri msg -j focused-output", &out, NULL, NULL,
                                 NULL) ||
      !out || !strstr(out, "\"logical\":") ||
      sscanf(strstr(out, "\"logical\":"), "\"logical\":{\"x\":%d,\"y\":%d", &x,
             &y) != 2)
    return first;
  for (guint i = 0; i < a->views->len; i++) {
    Notch *n = g_ptr_array_index(a->views, i);
    GdkRectangle box;
    gdk_monitor_get_geometry(n->monitor, &box);
    if (box.x == x && box.y == y)
      return n;
  }
  return first;
}

static int command_line(GApplication *app, GApplicationCommandLine *command,
                        gpointer data) {
  NotchApp *a = data;
  int argc;
  g_auto(GStrv) argv = g_application_command_line_get_arguments(command, &argc);
  if (argc == 2 && !strcmp(argv[1], "--quit")) {
    g_application_quit(app);
    return 0;
  }
  gboolean toggle = argc == 2 && !strcmp(argv[1], "--toggle");
  if (argc != 1 && !toggle) {
    g_application_command_line_printerr(
        command, "Usage: notch.sh [--toggle|--quit|--check]\n");
    return 2;
  }
  g_application_activate(app);
  if (!a->views || !a->views->len)
    return 1;
  if (toggle) {
    Notch *n = focused_view(a);
    pin_clicked(NULL, n);
    if (n->pinned) {
      // Only an explicit shortcut requests keyboard focus; hover never does.
      gtk_layer_set_keyboard_mode(GTK_WINDOW(n->window),
                                  GTK_LAYER_SHELL_KEYBOARD_MODE_EXCLUSIVE);
      gtk_widget_grab_focus(n->search);
    }
  }
  return 0;
}

static gboolean quit_signal(gpointer data) {
  g_application_quit(G_APPLICATION(data));
  return G_SOURCE_REMOVE;
}

static void view_cleanup(Notch *n) {
  if (n->flow_watch)
    g_bus_unwatch_name(n->flow_watch);
  g_clear_object(&n->flow_actions);
  g_clear_handle_id(&n->close_source, g_source_remove);
  g_clear_handle_id(&n->open_source, g_source_remove);
  g_clear_handle_id(&n->timer_source, g_source_remove);
  g_clear_handle_id(&n->media_source, g_source_remove);
  g_clear_handle_id(&n->watch_source, g_source_remove);
  g_clear_handle_id(&n->weather_source, g_source_remove);
  if (n->cancel)
    g_cancellable_cancel(n->cancel);
  g_clear_object(&n->cancel);
  g_clear_pointer(&n->blur, ext_background_effect_surface_v1_destroy);
  g_clear_object(&n->player);
  g_clear_object(&n->manager);
}

static void cleanup(NotchApp *a) {
  while (a->views && a->views->len)
    gtk_widget_destroy(((Notch *)g_ptr_array_index(a->views, 0))->window);
  g_clear_pointer(&a->views, g_ptr_array_unref);
  g_clear_object(&a->app);
}

int main(int argc, char **argv) {
  if (argc < 2 || !g_path_is_absolute(argv[1])) {
    g_printerr("Run through notch.sh.\n");
    return 2;
  }
  NotchApp n = {.scripts = argv[1], .timer = {.remaining = 1500}};
  memmove(argv + 1, argv + 2, (gsize)(argc - 1) * sizeof(char *));
  argc--;
  if (argc == 2 && !strcmp(argv[1], "--check")) {
    g_autoptr(GError) error = NULL;
    if (!load_style(n.scripts, &error)) {
      g_printerr("%s\n", error->message);
      return 1;
    }
    g_print("Native GTK, layer-shell, Playerctl and style ready\n");
    return 0;
  }
  if (!gtk_init_check(NULL, NULL) || !gtk_layer_is_supported()) {
    g_printerr(
        "Niri Notch requires a Wayland session with layer-shell support.\n");
    return 1;
  }
  n.app = gtk_application_new("dev.hype_niri.Notch",
                              G_APPLICATION_HANDLES_COMMAND_LINE);
  g_signal_connect(n.app, "activate", G_CALLBACK(activate), &n);
  g_signal_connect(n.app, "command-line", G_CALLBACK(command_line), &n);
  guint term_source = g_unix_signal_add(SIGTERM, quit_signal, n.app);
  guint int_source = g_unix_signal_add(SIGINT, quit_signal, n.app);
  int result = g_application_run(G_APPLICATION(n.app), argc, argv);
  // Signal handlers remove themselves when dispatched.
  if (g_main_context_find_source_by_id(NULL, term_source))
    g_source_remove(term_source);
  if (g_main_context_find_source_by_id(NULL, int_source))
    g_source_remove(int_source);
  cleanup(&n);
  return result;
}
