//! Runtime GTK styling for the sidebar.

use gtk4::gdk;

pub fn load_css() {
    let provider = gtk4::CssProvider::new();

    provider.load_from_data(
        "
        /* --- BASE WINDOW & ZONES --- */
        window {
            /* Dark, semi-transparent background (Catppuccin Base) */
            background-color: rgba(30, 30, 46, 0.95);
            color: #cdd6f4;
        }

        .zone {
            /* The 'Cards' that group buttons together */
            padding: 12px;
            background-color: rgba(255, 255, 255, 0.08); /* White with low opacity = Glass */
            border-radius: 12px;
            color: #cdd6f4;
        }

        /* --- BUTTONS (Circular & Squared) --- */
        /* Common resets to remove default GTK button styling */
        .circular-btn {
            border-radius: 99px; /* Pill/Circle shape */
            background-color: rgba(255, 255, 255, 0.1);
            color: white;
            border: none;
            box-shadow: none;
            background-image: none;
        }

        .squared-btn {
            border-radius: 8px;
            background-color: rgba(255, 255, 255, 0.1);
            color: white;
            border: none;
            box-shadow: none;
            padding: 0px;
            background-image: none;
        }

        /* Hover States */
        .circular-btn:hover, .squared-btn:hover {
            background-color: rgba(255, 255, 255, 0.2); /* Lighten on hover */
        }

        /* Active/Toggled States (e.g., Airplane Mode ON) */
        .circular-btn.active, .squared-btn.active {
            background-color: #3584e4; /* Gnome Blue */
            color: white;
            background-image: none;
        }

        .circular-btn.active:hover, .squared-btn.active:hover {
            background-color: #1c71d8; /* Darker Blue */
        }

        /* --- TYPOGRAPHY & UTILS --- */
        .badge {
            background-color: #ff5555; /* Red */
            color: white;
            border-radius: 99px;
            min-width: 14px;
            min-height: 14px;
            font-size: 10px;
            font-weight: bold;
            padding-left: 3px;
            padding-right: 3px;
            margin-top: -5px;  /* Nudge it up to overlap the icon */
            margin-right: -5px; /* Nudge it right */
        }

        .finance-text {
            font-size: 13px;
            font-weight: bold;
            /* Monospace font for aligned stock ticker numbers */
            font-family: 'JetBrainsMono Nerd Font', 'Roboto Mono', monospace;
        }

        .hint-text {
            font-size: 10px;
            color: alpha(white, 0.5); /* 50% Opacity */
        }

        /* --- CALENDAR WIDGET --- */
        .calendar-title {
            font-size: 16px;
            font-weight: bold;
            color: #89b4fa; /* Catppuccin Blue */
            margin-left: 10px;
            margin-right: 10px;
        }

        .calendar-header {
            font-size: 12px;
            color: alpha(white, 0.5);
            margin-bottom: 5px;
        }

        button.calendar-day-btn,
        button.calendar-day-btn:focus,
        button.calendar-day-btn:active {
            background-color: transparent;
            background-image: none;
            border: none;
            box-shadow: none;
            padding: 0px;
            border-radius: 8px;
        }
        
        .calendar-day-btn:hover {
            background-color: rgba(255, 255, 255, 0.1);
        }

        .calendar-day-num {
            font-size: 14px;
            font-weight: bold;
            color: #cdd6f4;
        }

        .calendar-dot {
            font-size: 10px;
            color: #f38ba8; /* Red dot for appointments */
            margin-top: -5px; /* Pull it up closer to number */
        }

        /* GTK themes often target button nodes directly, so we do the same here. */
        button.calendar-day-btn.today,
        button.calendar-day-btn.today:focus,
        button.calendar-day-btn.today:active,
        button.calendar-day-btn.today:checked {
            background-image: none;
            background-color: #89b4fa;
            border-radius: 8px;
        }

        button.calendar-day-btn.today:hover {
            background-image: none;
            background-color: #b4befe;
        }

        button.calendar-day-btn.today .calendar-day-num,
        button.calendar-day-btn.today .calendar-dot {
            color: #1e1e2e;
        }

        .calendar-switcher {
            margin-bottom: 4px;
        }

        .calendar-switcher button {
            background-image: none;
            background-color: rgba(255, 255, 255, 0.08);
            color: #cdd6f4;
            border: none;
            box-shadow: none;
            padding: 4px 12px;
        }

        .calendar-switcher button:first-child {
            border-radius: 8px 0 0 8px;
        }

        .calendar-switcher button:last-child {
            border-radius: 0 8px 8px 0;
        }

        .calendar-switcher button:checked {
            background-color: #89b4fa;
            color: #1e1e2e;
        }

        .calendar-switcher button:hover:not(:checked) {
            background-color: rgba(255, 255, 255, 0.16);
        }

        .calendar-nav-btn {
            color: #cdd6f4;
            min-width: 28px;
            min-height: 28px;
        }

        .calendar-nav-btn:hover {
            background-color: rgba(255, 255, 255, 0.12);
            border-radius: 8px;
        }

        .calendar-day-title {
            color: #89b4fa;
        }

        .agenda-event {
            color: #cdd6f4;
            padding: 4px 6px;
            border-radius: 6px;
        }

        .agenda-event:hover {
            background-color: rgba(255, 255, 255, 0.12);
        }

        .slider-icon {
            color: #cdd6f4;
        }

        scale.sidebar-slider trough {
            background-color: rgba(255, 255, 255, 0.18);
            border-radius: 999px;
            min-height: 6px;
        }

        scale.sidebar-slider highlight {
            background-color: #89b4fa;
            border-radius: 999px;
        }

        scale.sidebar-slider slider {
            background-color: #cdd6f4;
            border: none;
            box-shadow: none;
            min-width: 14px;
            min-height: 14px;
            border-radius: 999px;
        }

        /* Navigation Arrows and agenda rows */
        .flat {
            background: none;
            border: none;
            box-shadow: none;
        }

        /* --- MEDIA PLAYER CARD --- */
        .media-card {
            background-color: rgba(255, 255, 255, 0.08); /* Subtle glass effect */
            border-radius: 16px;
            padding: 20px;
            margin: 10px 20px;
            border: 1px solid rgba(255, 255, 255, 0.1);
        }

        .media-title {
            font-size: 18px;
            font-weight: bold;
            color: white;
            margin-bottom: 5px;
        }

        .media-artist {
            font-size: 14px;
            color: #cccccc;
            margin-bottom: 15px;
        }

        .media-btn {
            background: transparent;
            color: white;
            border: none;
            box-shadow: none;
            font-size: 24px;
            padding: 5px 15px;
            border-radius: 50%;
        }

        .media-btn:hover {
            background-color: rgba(255, 255, 255, 0.2);
        }

        .play-btn {
            font-size: 32px; /* Make Play/Pause slightly bigger */
            color: #89b4fa;  /* Accent color (Catppuccin Blueish) */
        }
        
        /* --- SYSTEM INFO CARD --- */
        .sysinfo-card {
            background-color: transparent;
            padding: 20px 40px; /* Extra side padding to center it visually */
            margin-top: 20px;
        }

        .sysinfo-key {
            font-size: 14px;
            font-weight: bold;
            color: #89b4fa; /* Catppuccin Blue */
            margin-bottom: 8px;
        }

        .sysinfo-value {
            font-size: 14px;
            font-weight: normal;
            color: #cdd6f4; /* Text White */
            margin-bottom: 8px;
        }
    ",
    );

    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
