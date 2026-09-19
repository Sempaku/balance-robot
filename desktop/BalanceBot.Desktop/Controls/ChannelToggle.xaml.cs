using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace BalanceBot.Desktop.Controls;

public partial class ChannelToggle : UserControl
{
    public static readonly DependencyProperty IsCheckedProperty =
        DependencyProperty.Register(nameof(IsChecked), typeof(bool), typeof(ChannelToggle), new PropertyMetadata(false));

    public static readonly DependencyProperty LabelProperty =
        DependencyProperty.Register(nameof(Label), typeof(string), typeof(ChannelToggle), new PropertyMetadata(string.Empty));

    public static readonly DependencyProperty ChannelColorProperty =
        DependencyProperty.Register(nameof(ChannelColor), typeof(Brush), typeof(ChannelToggle), new PropertyMetadata(Brushes.Gray));

    public bool IsChecked
    {
        get => (bool)GetValue(IsCheckedProperty);
        set => SetValue(IsCheckedProperty, value);
    }

    public string Label
    {
        get => (string)GetValue(LabelProperty);
        set => SetValue(LabelProperty, value);
    }

    public Brush ChannelColor
    {
        get => (Brush)GetValue(ChannelColorProperty);
        set => SetValue(ChannelColorProperty, value);
    }

    public ChannelToggle()
    {
        InitializeComponent();
    }
}
